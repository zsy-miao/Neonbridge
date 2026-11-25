"""Index Frosty's cooked shader archives and join the programs to runtime observations."""
from collections import defaultdict
from concurrent.futures import ThreadPoolExecutor
import csv
import ctypes
from ctypes.util import find_library
from dataclasses import dataclass
from functools import cache
import hashlib
import os
from pathlib import Path
import re
import struct
import subprocess
from tempfile import TemporaryDirectory
from threading import get_ident

from .inspect import digest

# The offsets locate uncompressed Pak v11 members in the audited shipping build.
ARCHIVES = {
    "global": (0x389806 + 53, 21_773_392, "afab84fa891e491d64f1c2a9bdf26a66afc2054aef2cd9110e358402473b4fb2"),
    "material": (0x19AB8C6, 340_714_881, "9de0439f815138c89fd414e69f4f5f3497fd523ac4dd2321c5207f7fd67eb2e1"),
}
STAGES = ("Vertex", "Mesh", "Amplification", "Pixel", "Geometry", "Compute",
          "Ray generation", "Ray miss", "Ray hit group", "Ray callable")
ENTRY = struct.Struct("<QIIB")
MAP = struct.Struct("<4I")
TRACE_SOURCE = re.compile(rb"TuxBox d3d12_xs: shader request (\w+) ([0-9a-f]{64})")
FIRST_SOURCE = re.compile(rb"TuxBox d3d12_xs: prepared (\w+) shader (?:loaded|missing or invalid), source SHA-256 ([0-9a-f]{64})")
COMPILED_SOURCE = re.compile(rb"\[TuxBox [^\]]+\] Compiled Xbox shader ([0-9a-f]{64})\s*$")
TITLE = "ae39eb1e92fa806df189c768c1478a53dfb54a2cc9769a9f2301ca7921e3eb04"


@dataclass(frozen=True)
class Entry:
    archive: str
    index: int
    stage: str
    shader_sha1: str
    source: str | None
    maps: tuple[int, ...]


def table(data, cursor, width):
    if cursor + 4 > len(data):
        raise ValueError("Truncated shader archive table count")
    count, = struct.unpack_from("<I", data, cursor)
    start = cursor + 4
    end = start + count * width
    if end > len(data):
        raise ValueError("Shader archive table exceeds the member")
    return data[start:end], end


def parse_tables(data):
    if data[:4] != b"\x02\0\0\0":
        raise ValueError("Expected a version 2 shader archive")
    cursor = 4
    tables = []
    for width in (20, 20, MAP.size, ENTRY.size, 16, 4):
        field, cursor = table(data, cursor, width)
        tables.append(field)
    map_ids, shader_ids, maps, programs, _, shader_indices = tables
    if len(map_ids) // 20 != len(maps) // MAP.size or len(shader_ids) // 20 != len(programs) // ENTRY.size:
        raise ValueError("Shader archive IDs and descriptors have different counts")
    indices = [item for item, in struct.iter_unpack("<I", shader_indices)]
    memberships = [[] for _ in range(len(shader_ids) // 20)]
    for group, (first, count, _, _) in enumerate(MAP.iter_unpack(maps)):
        if first + count > len(indices):
            raise ValueError("Shader map index range exceeds the table")
        for index in indices[first:first + count]:
            if index >= len(memberships):
                raise ValueError("Shader map references an unknown entry")
            memberships[index].append(group)
    return map_ids, shader_ids, programs, memberships, data[cursor:]


def dxbc(program):
    # A cooked raster/compute entry wraps exactly one complete DXBC. Some embed
    # the fourcc again inside a chunk, so use the container's declared boundary.
    start = program.find(b"DXBC")
    if start < 0 or start + 32 > len(program):
        raise ValueError("Cooked entry has no DXBC header")
    size, chunks = struct.unpack_from("<II", program, start + 24)
    if not chunks or not 32 + 4 * chunks <= size <= len(program) - start:
        raise ValueError("DXBC container exceeds its entry")
    return program[start:start + size]


def serialized_state(program):
    # Frosty's RHI passes the bytes between the SPTR label and the optional-data
    # trailer to DeserializeStateObjectX. The embedded DXBCs are root signatures.
    marker = program.find(b"SPTR")
    start = marker + 4
    if marker < 0 or len(program) < start + 4 + 14 or program[start:start + 4] != b"\x02\0\0\0":
        raise ValueError("Expected a packaged version 2 ray state")
    tail, = struct.unpack_from("<I", program, len(program) - 4)
    if tail != 14:
        raise ValueError("Unexpected shader optional-data length")
    return start, program[start:-tail]


@cache
def decoder():
    library = find_library("lz4")
    if not library:
        raise ValueError("liblz4 is required to read Frosty's compressed shader entries")
    decode = ctypes.CDLL(library).LZ4_decompress_safe
    decode.argtypes = [ctypes.c_char_p, ctypes.c_void_p, ctypes.c_int, ctypes.c_int]
    return decode


def archive_entries(name, stream, decode, survey, map_keys, *, include_ray_sources=False):
    offset, size, expected = ARCHIVES[name]
    stream.seek(offset)
    data = memoryview(stream.read(size))
    if len(data) != size or hashlib.sha256(data).hexdigest() != expected:
        raise ValueError(f"Unexpected {name} archive fingerprint")
    maps, ids, programs, memberships, payload = parse_tables(data)
    map_keys[name] = [maps[start:start+20].hex() for start in range(0, len(maps), 20)]
    for index, (start, stored, length, frequency) in enumerate(ENTRY.iter_unpack(programs)):
        if frequency >= len(STAGES) or not 0 < stored <= length or start + stored > len(payload):
            raise ValueError(f"Invalid {name} shader descriptor {index}")
        source = None
        if frequency < 6 or include_ray_sources:
            program = bytes(payload[start:start + stored])
            if stored != length:
                raw = ctypes.create_string_buffer(length)
                if decode(program, raw, stored, length) != length:
                    raise ValueError(f"Unable to decompress {name} shader {index}")
                program = raw.raw
            native = dxbc(program) if frequency < 6 else serialized_state(program)[1]
            source = hashlib.sha256(native).hexdigest()
            if survey:
                survey(source, native)
        yield Entry(name, index, STAGES[frequency], ids[20*index:20*(index+1)].hex(),
                    source, tuple(memberships[index]))


def requests(path):
    sources = {}
    traced = False
    compiled = False
    first = False
    fingerprint = hashlib.sha256()
    with path.open("rb") as stream:
        for line in stream:
            fingerprint.update(line)
            match = TRACE_SOURCE.search(line)
            if match:
                traced = True
            else:
                match = FIRST_SOURCE.search(line)
                first |= bool(match)
            if match:
                stage, source = (word.decode() for word in match.groups())
                if sources.get(source) not in (None, stage.title()):
                    raise ValueError(f"Conflicting stages for {source} in {path.name}")
                sources[source] = stage.title()
            elif match := COMPILED_SOURCE.search(line):
                # The worker logs standalone compilation even when per-request
                # tracing is off. The package provides its stage after indexing.
                compiled = True
                sources.setdefault(match[1].decode(), None)
    if not sources:
        raise ValueError(f"No TuxBox shader requests found in {path.name}")
    modes = []
    if traced:
        modes.append("per-source")
    elif first:
        modes.append("first hits and misses")
    if compiled:
        modes.append("standalone compilations")
    return sources, {"log_sha256": fingerprint.hexdigest(), "trace": ", ".join(modes),
                     "unique_sources": len(sources)}


def write_index(path, entries, profiled, phases, surveyed, validated, independent):
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", newline="", encoding="utf-8") as stream:
        out = csv.writer(stream)
        out.writerow(["archive", "entry", "stage", "shader_sha1", "source_dxbc_sha256", "shader_maps",
                      "in_preparation_profile", "observed_in", "hlsl_emitted", "standalone_dxil_validated",
                      "independent_hlsl"])
        for entry in entries:
            source = entry.source
            out.writerow([entry.archive, entry.index, entry.stage, entry.shader_sha1, source or "",
                          " ".join(map(str, entry.maps)), source in profiled,
                          " ".join(sorted(phases.get(source, ()))), surveyed.get(source, ""), validated.get(source, ""),
                          source in independent if independent is not None and source else ""])


def source_rows(report):
    yield from report.get("requested", ())
    for material in report.get("menu_materials", ()):
        yield from material["members"]


def use_frontend_survey(report, previous):
    """Carry a completed frontend survey forward when adding fresh runtime observations."""
    if (not previous.get("compiler_sha256") or previous["archives"] != report["archives"]
            or previous["executable_sha256"] != report["executable_sha256"]):
        raise ValueError("The saved frontend survey belongs to another build or has no compiler identity")
    measured = {row["stage"]: row for row in previous["stages"]}
    for row in report["stages"]:
        prior = measured.get(row["stage"], {})
        if prior.get("unique_dxbc") != row["unique_dxbc"]:
            raise ValueError(f"Frontend survey has a different {row['stage']} source count")
        emitted = prior["hlsl_emitted"]
        if row["unique_dxbc"] and (type(emitted) is not int or not 0 <= emitted <= row["unique_dxbc"]):
            raise ValueError(f"Invalid frontend count for {row['stage']}")
        row["hlsl_emitted"] = emitted
    known = {item["source_sha256"]: item["hlsl_emitted"] for item in source_rows(previous)
             if item.get("hlsl_emitted") is not None}
    for item in source_rows(report):
        item["hlsl_emitted"] = known.get(item["source_sha256"])
    report["compiler_sha256"] = previous["compiler_sha256"]
    if "independent_emitter" in previous:
        report["independent_emitter"] = previous["independent_emitter"]
    if previous.get("desktop_tools"):
        report["desktop_tools"] = previous["desktop_tools"]
        validated = {row["source_sha256"]: row.get("standalone_dxil_validated") for row in previous["requested"]}
        for row in report["requested"]:
            row["standalone_dxil_validated"] = validated.get(row["source_sha256"])


def survey_command(command):
    result = subprocess.run(command, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    if result.returncode < 0:
        raise RuntimeError(f"{Path(command[0]).name} terminated with signal {-result.returncode}")
    return result.returncode == 0


def desktop_tool_hashes(dxc):
    return {"dxc_sha256": digest(dxc), "dxv_sha256": digest(dxc.with_name("dxv"))}


def inventory(mount, plan, links, traces, compiler=None, index=None, materials=(), dxc=None, independent_compiler=None):
    executable = mount / "Frosty/Binaries/XSX/Frosty-XSX-Shipping.exe"
    if digest(executable) != TITLE or plan["executable_sha256"] != TITLE:
        raise ValueError("Frosty executable and preparation profile must match the reviewed build")
    profiled = {row["dxbc_sha256"]: row for row in plan["shaders"]}
    phases = defaultdict(set)
    trace_info, observed = {}, {}
    for label, path in traces.items():
        sources, trace_info[label] = requests(path)
        for source, stage in sources.items():
            previous = observed.get(source)
            if stage and previous and previous != stage:
                raise ValueError(f"Conflicting stage for {source}")
            observed[source] = stage or previous
            phases[source].add(label)
    entries, surveyed, validated, map_keys = [], {}, {}, {}
    compiler = compiler.resolve() if compiler else None
    dxc = dxc.resolve() if dxc else None
    compiler_hash = digest(compiler) if compiler else None
    if (dxc or independent_compiler) and not compiler:
        raise ValueError("The independent and desktop surveys require the regular Xbox compiler")
    independent = set() if independent_compiler else None
    if independent_compiler:
        independent_compiler = independent_compiler.resolve()
        independent_hash = digest(independent_compiler)
        modes = subprocess.run([independent_compiler, "--list-emitters"], capture_output=True)
        if modes.returncode or modes.stdout.splitlines() != [b"independent"]:
            raise ValueError("--independent-compiler requires an xshader built with --no-default-features")
    desktop_tools = desktop_tool_hashes(dxc) if dxc else None
    with TemporaryDirectory(prefix="tuxbox-shader-survey-") as tmp, ThreadPoolExecutor(max_workers=4) as workers:
        batch = {}

        def translate(item):
            source, native = item
            work = Path(tmp) / str(get_ident())
            work.mkdir(exist_ok=True)
            sample, hlsl, profile, desktop = (work / file for file in ("shader.dxbc", "shader.hlsl", "profile", "output.cso"))
            sample.write_bytes(native)
            own = False
            if independent_compiler:
                own_hlsl, own_profile = work / "independent.hlsl", work / "independent.profile"
                own_hlsl.unlink(missing_ok=True)
                own_profile.unlink(missing_ok=True)
                own = survey_command([independent_compiler, sample, "--hlsl-out", own_hlsl, "--profile-out", own_profile])
            needs_dxil = dxc and (source in observed or own)
            if not (own or needs_dxil):
                return source, survey_command([compiler, sample, "--hlsl-out", os.devnull]), None, False
            for output in (hlsl, profile, desktop):
                output.unlink(missing_ok=True)
            emitted = survey_command([compiler, sample, "--hlsl-out", hlsl, "--profile-out", profile])
            if own and (not emitted or own_hlsl.read_bytes() != hlsl.read_bytes()
                        or own_profile.read_bytes() != profile.read_bytes()):
                raise ValueError(f"Independent and regular compilers disagree for {source}")
            valid = None
            if needs_dxil:
                compiled = emitted and survey_command(
                    [dxc, "-T", profile.read_text().strip(), "-E", "main", "-Fo", desktop, hlsl])
                valid = compiled and survey_command([dxc.with_name("dxv"), desktop])
            return source, emitted, valid, own

        def finish_batch():
            for source, emitted, valid, own in workers.map(translate, batch.items()):
                surveyed[source] = emitted
                if own:
                    independent.add(source)
                if valid is not None:
                    validated[source] = valid
            batch.clear()

        def enqueue(source, native):
            if source not in surveyed:
                batch.setdefault(source, native)
            # Bound queued bytecode even for the largest archive.
            if len(batch) == 32:
                finish_batch()

        decode = decoder()
        with (mount / "Frosty/Content/Paks/Frosty-XSX.pak").open("rb") as stream:
            for name in ARCHIVES:
                entries.extend(archive_entries(name, stream, decode, enqueue if compiler else None, map_keys))
        finish_batch()
    if ((compiler and digest(compiler) != compiler_hash) or (dxc and desktop_tool_hashes(dxc) != desktop_tools)
            or (independent_compiler and digest(independent_compiler) != independent_hash)):
        raise ValueError("The shader tools changed during the survey")

    locations = defaultdict(list)
    by_id = {}
    for entry in entries:
        if entry.source:
            locations[entry.source].append(entry)
        by_id[(entry.archive, entry.index)] = entry
    for source, row in profiled.items():
        if by_id[(row["archive"], row["entry"])].source != source:
            raise ValueError(f"Profile digest differs at {row['archive']} entry {row['entry']}")
    for source, stage in observed.items():
        if stage is None:
            stage = next((entry.stage for entry in locations[source]), "Unknown")
            observed[source] = stage
        if any(entry.stage != stage for entry in locations.get(source, ())):
            raise ValueError(f"The trace and archive disagree on stage for {source}")
    for pair in links:
        if pair["producer"] not in profiled or pair["pixel"] not in profiled:
            raise ValueError("Linked shader refers to an absent source profile")
    if index:
        write_index(index, entries, profiled, phases, surveyed, validated, independent)
    stages = []
    for stage in STAGES:
        rows = [row for row in entries if row.stage == stage]
        if not rows and stage not in observed.values():
            continue
        native = {row.source for row in rows if row.source}
        requested = {source for source, value in observed.items() if value == stage}
        stages.append({"stage": stage, **{name: sum(row.archive == name for row in rows) for name in ARCHIVES},
                       "unique_dxbc": len(native), "profiled_dxbc": len(native & profiled.keys()),
                       "hlsl_emitted": sum(surveyed[key] for key in native) if compiler and native else None,
                       "requested_dxbc": len(requested), "requested_in_profile": len(requested & profiled.keys())})
    required = []
    for source in sorted(observed, key=lambda key: (observed[key], key)):
        row = {"stage": observed[source], "source_sha256": source, "observed_in": sorted(phases[source]),
               "archive_entries": [f"{e.archive}/{e.index}" for e in locations.get(source, ())],
               "in_preparation_profile": source in profiled, "hlsl_emitted": surveyed.get(source),
               "standalone_dxil_validated": validated.get(source)}
        if source in profiled:
            row["profile"] = profiled[source]["profile"]
        required.append(row)
    groups = []
    for material in materials:
        group = material["shader_map_entry"]
        if map_keys["material"][group] != material["shader_map_sha1"]:
            raise ValueError(f"Material shader map differs for {material['asset']}")
        members = [row for row in entries if row.archive == "material" and group in row.maps]
        if sorted(material["entries"]) != sorted(row.index for row in members):
            raise ValueError(f"Material shader map membership differs for {material['asset']}")
        groups.append({"asset": material["asset"], "map": group, "shader_map_sha1": map_keys["material"][group],
                       "members": [{"entry": row.index, "stage": row.stage, "source_sha256": row.source,
                                    "in_preparation_profile": row.source in profiled,
                                    "hlsl_emitted": surveyed.get(row.source),
                                    "observed_in": sorted(phases.get(row.source, ()))} for row in members]})
    report = {"title": "Frosty", "executable_sha256": TITLE,
            "archives": {name: {"member_sha256": meta[2], "entries": sum(row.archive == name for row in entries)}
                         for name, meta in ARCHIVES.items()},
            "compiler_sha256": compiler_hash, "desktop_tools": desktop_tools, "traces": trace_info, "stages": stages,
            "linked_variants": len(links), "menu_materials": groups, "requested": required}
    if independent_compiler:
        report["independent_emitter"] = {
            "compiler_sha256": independent_hash,
            "programs": [{"source_sha256": key, "stage": locations[key][0].stage,
                          "standalone_dxil_validated": validated.get(key)} for key in sorted(independent)]
        }
    return report
