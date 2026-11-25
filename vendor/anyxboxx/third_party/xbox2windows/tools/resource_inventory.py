"""Read layout targets, file signatures and shader-format evidence without extracting assets."""

import argparse
from collections import Counter
import hashlib
import json
import mmap
from pathlib import Path
import re
import struct
import subprocess
from tempfile import NamedTemporaryFile
import zlib

from verify_build import read_layout


def signature(header):
    if header.removeprefix(b"\xef\xbb\xbf").startswith(b"<?xml "):
        return "XML declaration"
    if header.startswith(b"\x89PNG\r\n\x1a\n"):
        return "PNG"
    if header.startswith(b"MZ") and len(header) >= 64:
        pe_offset, = struct.unpack_from("<I", header, 60)
        if header[pe_offset:pe_offset + 4] == b"PE\0\0":
            return "PE"
    if header[512:520] == b"msft-xvd":
        return "msft-xvd at +512"
    for tag in (b"9FPR", b"RDS5", b"mrm_pri2", b"regf"):
        if header.startswith(tag):
            return tag.decode("ascii")
    return "unclassified"


def _parts_overlap(spans):
    """Compare physical part extents, independent of offset-table ordering."""
    ordered = sorted(spans)
    return any(left[1] > right[0] for left, right in zip(ordered, ordered[1:]))


def parts(blob):
    if len(blob) < 32 or blob[:4] != b"DXBC":
        raise ValueError("Expected a DXBC container")
    version, size, count = struct.unpack_from("<3I", blob, 20)
    table_end = 32 + 4 * count
    if version != 1 or size != len(blob) or table_end > size:
        raise ValueError("Container header disagrees with inventory")
    result = {}
    spans = []
    for index in range(count):
        offset, = struct.unpack_from("<I", blob, 32 + 4 * index)
        if offset < table_end or offset + 8 > size:
            raise ValueError("Invalid container part offset")
        tag, length = struct.unpack_from("<4sI", blob, offset)
        part_end = offset + 8 + length
        if part_end > size or tag in result:
            raise ValueError("Invalid container part")
        spans.append((offset, part_end))
        result[tag] = blob[offset + 8:part_end]
    if _parts_overlap(spans):
        raise ValueError("Overlapping container parts")
    return result


def dxil_program(data, offset):
    """Check container/part extents and DXIL bitcode framing; no shader execution."""
    if offset < 0 or len(data) - offset < 32 or data[offset:offset + 4] != b"DXBC":
        return False
    version, size, parts = struct.unpack_from("<III", data, offset + 20)
    if version != 1 or size > len(data) - offset or not 0 < parts <= (size - 32) // 4:
        return False
    table_end = 32 + parts * 4
    spans = []
    seen_tags = set()
    program = False
    for i in range(parts):
        relative, = struct.unpack_from("<I", data, offset + 32 + 4 * i)
        if relative < table_end or relative + 8 > size:
            return False
        start = offset + relative
        tag, extent = struct.unpack_from("<4sI", data, start)
        part_end = relative + 8 + extent
        if part_end > size or tag in seen_tags:
            return False
        seen_tags.add(tag)
        spans.append((relative, part_end))
        if tag == b"DXIL" and extent >= 24:
            body = start + 8
            _, words, magic, _, delta, length = struct.unpack_from("<II4sIII", data, body)
            bitcode = body + 8 + delta
            if (magic == b"DXIL" and delta >= 16 and length >= 4 and
                    8 + delta + length <= words * 4 <= extent and
                    data[bitcode:bitcode + 4] == b"BC\xc0\xde"):
                program = True
    return program and not _parts_overlap(spans)


def shader_markers(data):
    counters = {"DXBC": 0, "DXIL": 0, "framed_dxil_programs": 0}
    for tag in ("DXBC", "DXIL"):
        token = tag.encode("ascii")
        offset = data.find(token)
        while offset >= 0:
            counters[tag] += 1
            if tag == "DXBC" and dxil_program(data, offset):
                counters["framed_dxil_programs"] += 1
            offset = data.find(token, offset + 4)
    return counters


def probe_archive(path):
    # A limited probe inspired by published observations for common/data RPF9.
    prefix_limit, input_limit, output_limit = 4 << 20, 512 << 10, 2 << 20
    with path.open("rb") as stream:
        if not path.stat().st_size:
            raise ValueError(f"Empty archive: {path.name}")
        with mmap.mmap(stream.fileno(), 0, access=mmap.ACCESS_READ) as data:
            raw = shader_markers(data)
            prefix = data[:prefix_limit]
    streams = []
    for i in range(0, len(prefix) - 1, 16):
        if prefix[i] != 0x78 or prefix[i + 1] & 32 or (0x7800 + prefix[i + 1]) % 31:
            continue
        sample = prefix[i:i + input_limit]
        decoder = zlib.decompressobj()
        try:
            decoded = decoder.decompress(sample, output_limit)
        except zlib.error:
            continue
        if decoder.eof:
            streams.append({"offset": i, "compressed_bytes": len(sample) - len(decoder.unused_data),
                            "decoded_bytes": len(decoded), "decoded_markers": shader_markers(decoded)})
        if len(streams) == 8:
            break
    return {"raw_bytes_scanned": path.stat().st_size, "raw_markers": raw, "sample_prefix_bytes": len(prefix),
            "sample_streams": streams}


def scene_records(data, validator=None, *, observe=None):
    """Summarize candidate named raw-DEFLATE shader records without exporting their contents."""
    names = re.compile(rb"[!-~]{3,}")
    stages = Counter()
    sections_seen = Counter()
    tails = Counter()
    rejected = Counter()
    samples = {}
    framed = 0
    cursor = 0
    while match := names.search(data, cursor):
        cursor = match.end()
        if cursor + 18 > len(data) or data[cursor] != 0:
            continue
        stored, _, repeated, decoded = struct.unpack_from("<4I", data, cursor + 2)
        if not (16 < stored <= 8 << 20 and 32 <= decoded <= 16 << 20):
            continue
        if stored != repeated:
            rejected["unequal_stored_lengths"] += 1
            continue
        start = cursor + 18
        if start + stored > len(data):
            rejected["truncated_span"] += 1
            continue
        stream = zlib.decompressobj(-15)
        try:
            blob = stream.decompress(data[start:start+stored], decoded + 1)
        except zlib.error:
            rejected["deflate_error"] += 1
            continue
        if len(blob) != decoded or not stream.eof:
            rejected["decoded_length_or_eof"] += 1
            continue
        try:
            sections = parts(blob)
        except ValueError:
            rejected["container_framing"] += 1
            continue
        program = sections.get(b"DXIL", b"")
        if len(program) < 24 or program[8:12] != b"DXIL":
            rejected["no_program_header"] += 1
            continue
        kind = struct.unpack_from("<H", program, 2)[0]
        stage = {0:"pixel", 1:"vertex", 2:"geometry", 3:"hull", 4:"domain", 5:"compute",
                 6:"library", 13:"mesh", 14:"amplification"}.get(kind, f"kind_{kind}")
        stages[stage] += 1
        if observe is not None:
            observe(kind, blob, sections)
        sections_seen.update(tag.decode("ascii", "replace") for tag in sections)
        tails[len(stream.unused_data)] += 1
        framed += dxil_program(blob, 0)
        if validator and stage not in samples:
            run = subprocess.run([validator, "--container"], input=blob, capture_output=True,
                                 timeout=15, check=True)
            result = json.loads(run.stdout)
            samples[stage] = {"call_hr": result["call_hr"], "status": result.get("status")}
        cursor = start + (stored - len(stream.unused_data))
    return {"records": sum(stages.values()), "stages": stages, "section_counts": sections_seen,
            "framed_bitcode": framed, "trailing_byte_counts": tails, "candidate_rejections": rejected,
            "validator_samples": len(samples), "first_per_stage": samples}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path)
    parser.add_argument("--shader-control", type=Path)
    parser.add_argument("--scene-records", action="store_true", help="Inspect compressed records in shaders.rpf")
    parser.add_argument("--scene-validator", type=Path, help="Use the built root_probe.exe on one container per stage")
    args = parser.parse_args()
    root = args.directory.resolve(strict=True)
    layouts = [path for path in root.glob("layout*.xml") if path.is_file()]
    if len(layouts) != 1:
        parser.error(f"expected exactly one layout*.xml file in {root}, found {len(layouts)}")
    layout, = layouts
    markers, targets = read_layout(layout)
    rows = []
    for path in sorted(root.rglob("*")):
        if not path.is_file() or path.suffix.lower() == ".torrent":
            continue
        with path.open("rb") as stream:
            header = stream.read(1024)
        name = path.relative_to(root).as_posix()
        row = {"path": name, "bytes": path.stat().st_size, "chunks": targets.get(name, []),
               "prefix_hex": header[:8].hex(), "signature": signature(header)}
        if row["signature"] == "PE" and path.suffix.lower() == ".bin":
            pe_offset, = struct.unpack_from("<I", header, 60)
            if pe_offset + 44 <= len(header):
                machine, sections = struct.unpack_from("<HH", header, pe_offset + 4)
                row["pe_machine"] = hex(machine)
                row["pe_sections"] = sections
                row["pe_entry_rva"], = struct.unpack_from("<I", header, pe_offset + 40)
        rows.append(row)
    available = {r["path"] for r in rows}
    chunks = {}
    for chunk, marker in markers.items():
        files = [p for p in targets if chunk in targets[p]]
        chunks[chunk] = {"marker": marker, "targets": files,
                         "present_bytes": sum(r["bytes"] for r in rows if chunk in r["chunks"]),
                         "missing": sorted(set(files) - available)}
    control = None
    if args.shader_control:
        data = args.shader_control.read_bytes()
        control = {"file": args.shader_control.name, "sha256": hashlib.sha256(data).hexdigest(),
                   "markers": shader_markers(data)}
        if not control["markers"]["framed_dxil_programs"]:
            raise ValueError("Positive control has no structurally recognizable DXIL program")
    focus = {name: probe_archive(root / name) for name in ("appdata0.rpf", "common.rpf", "shaders.rpf")
             if name in available}
    report = {
        "build": root.name, "layout": layout.name, "chunks": chunks,
        "signature_counts": dict(Counter(r["signature"] for r in rows)), "files": rows,
        "outside_layout": [r["path"] for r in rows if not r["chunks"]],
        "scope": "Layout references describe installation; runtime reads and inner archive directories are unknown.",
        "probe_scope": "Raw marker search covers the three entire archives. Sample up to 8 checksum-valid zlib streams at 16-byte aligned 78xx headers in first 4 MiB; limits 512 KiB input and 2 MiB output per candidate. No inference about remaining encoded content.",
        "positive_control": control, "archive_probes": focus,
    }
    if args.scene_records or args.scene_validator:
        with (root / "shaders.rpf").open("rb") as stream:
            if stream.seek(0, 2):
                with mmap.mmap(stream.fileno(), 0, access=mmap.ACCESS_READ) as data:
                    report["scene_records"] = scene_records(data, args.scene_validator)
            else:
                report["scene_records"] = scene_records(b"")
    output = Path(__file__).resolve().parents[1] / "out" / "resources" / (root.name + ".json")
    if output.resolve().is_relative_to(root):
        raise ValueError("Resource report must be outside the game directory")
    output.parent.mkdir(parents=True, exist_ok=True)
    with NamedTemporaryFile(dir=output.parent, mode="w", encoding="utf-8", delete=False) as f:
        json.dump(report, f, indent=2, ensure_ascii=False)
        f.write("\n")
    Path(f.name).replace(output)
    print(f"{len(chunks)} chunks | {len(rows)} files | {output}")
    for name, probe in focus.items():
        print(name, probe["raw_markers"], "sampled zlib streams", len(probe["sample_streams"]))


if __name__ == "__main__":
    main()
