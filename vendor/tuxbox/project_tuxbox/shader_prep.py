"""Prepare profiled retail shaders in the cache read by the staged graphics provider."""
import ctypes
import hashlib
import json
import os
from pathlib import Path
import shutil
import struct
import subprocess
import sys
from tempfile import TemporaryDirectory
from time import monotonic

from .inspect import digest
from .prep_progress import PrepPhase, desktop_progress, report_progress
from .shader_inventory import decoder


ROOT = Path(__file__).resolve().parents[1]
FROSTY_PLAN = ROOT / "tools/reference/frosty-first-boot.json"
PROFILES = {"frosty-xsx-shipping.exe": FROSTY_PLAN}
GUEST_TEMP = Path("drive_c/users/steamuser/AppData/Local/Temp")
SOURCE_CACHE = GUEST_TEMP / "tuxbox_shader_cache/by_source"
TUXBOX_PROVIDER = ROOT / "build/d3d12-smoke/d3d12_xs.dll"


def cooked_shader(pak, record):
    """Read one profiled, possibly LZ4-packed program from the local retail package."""
    with pak.open("rb") as stream:
        stream.seek(record["offset"])
        source = stream.read(record["packed_bytes"])
    if len(source) != record["packed_bytes"]:
        raise ValueError(f"Truncated shader entry {record['entry']} in {pak}")
    size = record["unpacked_bytes"]
    if len(source) != size:
        decode = decoder()
        target = ctypes.create_string_buffer(size)
        if decode(source, target, len(source), size) != size:
            raise ValueError(f"Cannot decompress shader entry {record['entry']}")
        source = target.raw
    start = record["dxbc_offset"]
    cooked = source[start:start + record["dxbc_bytes"]]
    if hashlib.sha256(cooked).hexdigest() != record["dxbc_sha256"]:
        raise ValueError(f"Package shader differs at {record['archive']} entry {record['entry']}")
    return cooked


def shader_compiler():
    override = os.environ.get("TUXBOX_XSHADER")
    if override:
        xshader = shutil.which(override)
        if not xshader:
            raise ValueError(f"TUXBOX_XSHADER executable was not found: {override}")
    else:
        info = subprocess.check_output(["rustc", "-vV"], text=True)
        target = next(line.removeprefix("host: ") for line in info.splitlines() if line.startswith("host: "))
        output = ROOT / "build/shader-compiler"
        run_tool(["cargo", "build", "--release", "--locked", "--target", target,
                  "--manifest-path", ROOT / "tools/shader-compiler/Cargo.toml", "--target-dir", output])
        xshader = output / target / "release/xshader"
    return Path(xshader)


def compiler_tools():
    from build import find_dxc

    xshader = shader_compiler()
    dxc = Path(find_dxc(None))
    dxv = dxc.with_name("dxv")
    if not dxv.is_file():
        dxv = shutil.which("dxv")
    if not dxv:
        raise ValueError("Install the DXIL validator (dxv) from the DirectX Shader Compiler toolchain")
    return xshader, dxc, dxv


class ToolFailure(ValueError):
    def __init__(self, tool, returncode, detail):
        super().__init__(f"{Path(tool).name} failed ({returncode}): {detail}")
        self.returncode = returncode
        self.tool = Path(tool).name


def run_tool(command, *, timeout=None):
    result = subprocess.run(command, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE,
                            text=True, errors="replace", timeout=timeout)
    if result.returncode:
        raise ToolFailure(command[0], result.returncode, result.stderr[-600:].strip())


def compile_shader(pak, record, tools, work):
    xshader, dxc, dxv = tools
    cooked, hlsl, dxil = (work / name for name in ("shader.dxbc", "shader.hlsl", "shader.cso"))
    cooked.write_bytes(cooked_shader(pak, record))
    hlsl.unlink(missing_ok=True)
    arguments = [xshader, cooked, "--hlsl-out", hlsl]
    if pixel := record.get("pixel_input"):
        pixel_source = work / "pixel.dxbc"
        pixel_source.write_bytes(cooked_shader(pak, pixel))
        arguments.extend(["--link-pixel", pixel_source])
        if order := record.get("pixel_order"):
            arguments.extend(["--pixel-order", order])
    run_tool(arguments)
    if not hlsl.is_file() or digest(hlsl) != record["hlsl_sha256"]:
        raise ValueError(f"xshader translation differs from the staged provider for entry {record['entry']}")
    run_tool([dxc, "-T", record["profile"], "-E", "main", "-Fo", dxil, hlsl])
    run_tool([dxv, dxil])
    return dxil


def has_dxil_container(data):
    """Check the container and bitcode bounds without depending on compiler tools."""
    if len(data) < 32 or data[:4] != b"DXBC":
        return False
    size, count = struct.unpack_from("<II", data, 24)
    toc_end = 32 + count * 4
    if not count or not toc_end <= size <= len(data):
        return False
    found = False
    for offset, in struct.iter_unpack("<I", data[32:toc_end]):
        if offset < toc_end or offset + 8 > size:
            return False
        tag, length = struct.unpack_from("<4sI", data, offset)
        start = offset + 8
        if start + length > size:
            return False
        if tag == b"DXIL":
            if length < 24 or data[start + 8:start + 12] != b"DXIL":
                return False
            bit_offset, bit_size = struct.unpack_from("<II", data, start + 16)
            bit_start = 8 + bit_offset
            if bit_size < 4 or bit_start + bit_size > length:
                return False
            found = data[start + bit_start:start + bit_start + 4] == b"BC\xc0\xde"
    return found


def checked_entries(cache, records, previous, *, exact_sources=False):
    """Remove structurally broken programs from the provider's lookup path even if compilation fails."""
    outputs = {}
    for key, record in records.items():
        entry = cache / (key + ".cso")
        try:
            if exact_sources:
                actual = digest(entry)
            else:
                data = entry.read_bytes()
                actual = hashlib.sha256(data).hexdigest()
        except FileNotFoundError:
            continue
        if actual in (previous.get(key), record["observed_dxil_sha256"], record.get("compiled_dxil_sha256")):
            outputs[key] = actual
        elif exact_sources or not has_dxil_container(data):
            # Keep a recoverable copy outside the runtime's cache lookup path.
            quarantine = cache.parent / "tuxbox-invalid"
            quarantine.mkdir(exist_ok=True)
            os.replace(entry, quarantine / f"{key}.{actual}.cso")
    return outputs


def verified_copy(source, allowed, work):
    """Take a snapshot and check its digest before publishing to a different cache."""
    try:
        with source.open("rb") as stream:
            size = os.fstat(stream.fileno()).st_size
            if size > 64 << 20:  # Also the D3D12 provider's maximum shader size.
                return None
            data = stream.read(size + 1)
    except FileNotFoundError:
        return None
    if len(data) != size or hashlib.sha256(data).hexdigest() not in allowed:
        return None
    candidate = work / "copy.cso"
    candidate.write_bytes(data)
    return candidate


def prepare_shaders(pak, plan, cache, progress=None, *, linked=False, exact_sources=False):
    """Prepare checked DXIL entries and deduplicate equivalent source translations."""
    import fcntl  # The profiled cache layout belongs to the Linux Proton backend.

    cache.mkdir(parents=True, exist_ok=True)
    state = (cache / "tuxbox-first-boot.json") if linked else (cache.parent / "tuxbox-first-boot.json")
    # Keep this inside the provider cache so a cache reset also resets the receipt.
    with (cache.parent / "tuxbox-first-boot.lock").open("a+b") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        version = hashlib.sha256(json.dumps(plan, sort_keys=True).encode()).hexdigest()
        try:
            receipt = json.loads(state.read_text())
        except (FileNotFoundError, UnicodeDecodeError, json.JSONDecodeError):
            receipt = {}
        previous = receipt.get("outputs", {}) if isinstance(receipt, dict) and receipt.get("plan") == version else {}
        if not isinstance(previous, dict):
            previous = {}
        compiled = 0
        tools = None
        with TemporaryDirectory(dir=cache.parent, prefix=".prepare-") as directory:
            work = Path(directory)
            key_field = "pair_key" if linked else ("dxbc_sha256" if exact_sources else "cache_key")
            unique = {record[key_field]: record for record in plan["shaders"]}
            label = "Linked menu shaders" if linked else "Startup and menu shaders"
            phase = PrepPhase(progress, label, len(unique))
            # Inspect every file before looking for tools; one missing compiler
            # must not leave later truncated entries in the runtime cache.
            outputs = checked_entries(cache, unique, previous, exact_sources=exact_sources)
            published = {}
            for ordinal, (key, record) in enumerate(unique.items(), 1):
                entry = cache / (key + ".cso")
                translation = (record.get("hlsl_sha256"), record["profile"])
                if exact_sources:
                    # The executable hash alone cannot authenticate the separate .pak.
                    cooked_shader(pak, record)
                    if linked:
                        cooked_shader(pak, record["pixel_input"])
                if key in outputs:
                    published[translation] = (entry, outputs[key])
                    phase.update(ordinal, compiled)
                    continue
                dxil = None
                if exact_sources:
                    if translation in published:
                        source, sha = published[translation]
                        dxil = verified_copy(source, {sha}, work)
                if dxil is None:
                    if tools is None:
                        tools = compiler_tools()
                    dxil = compile_shader(pak, record, tools, work)
                    compiled += 1
                outputs[key] = digest(dxil)
                os.replace(dxil, entry)
                published[translation] = (entry, outputs[key])
                # Preserve completed work if a later compile fails or is interrupted.
                pending = work / "receipt.json"
                pending.write_text(json.dumps({"plan": version, "outputs": {**previous, **outputs}}) + "\n")
                os.replace(pending, state)
                phase.update(ordinal, compiled)
            if receipt != {"plan": version, "outputs": outputs}:
                pending = work / "receipt.json"
                pending.write_text(json.dumps({"plan": version, "outputs": outputs}) + "\n")
                os.replace(pending, state)
        return compiled, len(outputs) - compiled


def matching_tuxbox_provider(provider, sha=None):
    if not provider.is_file():
        return False
    if provider.parts[-5:-1] == ("Mount", "Frosty", "Binaries", "XSX"):
        work = provider.parents[4]
        manifest = work / "providers.json"
        if manifest.is_file():
            record = json.loads(manifest.read_text()).get(str(provider.relative_to(work)), {})
            return record.get("origin") == "TuxBox d3d12" and record.get("sha256") == (sha or digest(provider))
    # The packaged sample keeps its own copy across rebuilds of the development DLL.
    for built in (TUXBOX_PROVIDER, ROOT / "build/lanes/d3d12_xs.dll"):
        if built.is_file() and provider.stat().st_size == built.stat().st_size:
            if sha is None and provider.samefile(built):
                return True
            sha = sha or digest(provider)
            if sha == digest(built):
                return True
    return False


def tuxbox_plan(profile_path, raw_plan, include_menu):
    """Expand source aliases and the requested launch profile from package metadata."""
    plan = dict(raw_plan)
    source_profile = plan.pop("source_cache")
    compiled_hashes = source_profile["compiled_dxil_sha256"]
    programs = {record["cache_key"]: [dict(record, compiled_dxil_sha256=compiled_hashes[record["cache_key"]])]
               for record in plan["shaders"] if record["cache_key"] in compiled_hashes}
    additional = list(source_profile.get("additional_shaders", ()))
    if include_menu and source_profile.get("menu_profile"):
        additional.extend(json.loads(profile_path.with_name(source_profile["menu_profile"]).read_text()))
    for record in additional:
        programs.setdefault(record["cache_key"], []).append(record)
    for alias in source_profile["aliases"]:
        group = programs[alias["cache_key"]]
        group.append({**group[0], **alias})
    # Publish each verified alias alongside its translation even if compiling
    # an unrelated, later shader is interrupted.
    plan["shaders"] = [record for group in programs.values() for record in group]
    if not include_menu and source_profile.get("linked_shaders"):
        sources = {row["dxbc_sha256"] for row in plan["shaders"]}
        source_profile = {**source_profile, "linked_shaders": [
            pair for pair in source_profile.get("linked_shaders", ())
            if pair["producer"] in sources and pair["pixel"] in sources
        ]}
    return plan, source_profile


def prepare_for_launch(executable, host):
    """Return False when the selected target has no matching preparation profile."""
    if host.backend != "proton":
        return False
    if executable.name.casefold() == "game_scarlett_master.exe":
        from .scarlett_prep import prepare_scarlett

        return prepare_scarlett(executable, host)
    profile = PROFILES.get(executable.name.lower())
    if profile is None:
        return False
    plan = json.loads(profile.read_text())
    provider = executable.with_name(plan["provider"])
    if not matching_tuxbox_provider(provider):
        return False
    if digest(executable) != plan["executable_sha256"]:
        return False
    mode = os.environ.get("TUXBOX_SHADER_PREP", "chase")
    if mode not in ("0", "1", "menu", "chase"):
        print(f"Unknown TUXBOX_SHADER_PREP={mode!r}. Using chase.", file=sys.stderr)
        mode = "chase"
    name = mode if mode in ("menu", "chase") else "startup"
    print(f"{plan['title']} shader preparation profile: {name}", file=sys.stderr, flush=True)
    plan, source_profile = tuxbox_plan(profile, plan, mode in ("menu", "chase"))
    pak = executable.parent / plan["shader_package"]
    prefix = Path(host.env["WINEPREFIX"])
    cache = prefix / SOURCE_CACHE
    started = monotonic()
    with desktop_progress(plan["title"], host.env) as progress:
        compiled, reused = prepare_shaders(pak, plan, cache, progress=progress,
                                          exact_sources=True)
        if source_profile.get("linked_shaders"):
            sources = {row["dxbc_sha256"]: row for row in plan["shaders"]}
            pairs = {}
            for link in source_profile["linked_shaders"]:
                producer, pixel = sources[link["producer"]], sources[link["pixel"]]
                directory = {"gs_6_0": "by_gs_ps", "vs_6_0": "by_vs_ps"}[producer["profile"]]
                pair = {**producer, **link, "pixel_input": pixel,
                        "pair_key": f"{producer['dxbc_sha256']}_{pixel['dxbc_sha256']}"}
                if link.get("ordered"):
                    pair["pixel_order"] = str(cache / (pixel["dxbc_sha256"] + ".cso"))
                pairs.setdefault(directory, []).append(pair)
            for directory, records in pairs.items():
                pair_plan = {"title": plan["title"], "shaders": records}
                built, found = prepare_shaders(pak, pair_plan, cache.parent / directory, progress=progress,
                                               exact_sources=True, linked=True)
                compiled += built
                reused += found
        if mode == "chase":
            from .chase_warmup import prepare_chase
            from .pipeline_prep import prepare_pipelines
            chase_profile = json.loads(profile.with_name("frosty-chase-warmup.json").read_text())
            built, found = prepare_chase(pak, cache, chase_profile, plan["executable_sha256"],
                                         {row["dxbc_sha256"] for row in plan["shaders"]}, progress)
            compiled += built
            reused += found
            prepare_pipelines(executable, host, plan, cache, progress, standalone=True)
        elapsed = monotonic() - started
        report_progress(progress, f"{plan['title']} shaders ready: {compiled} compiled, {reused} cached. "
                        f"Preparation took {elapsed:.1f}s.", 1, 1)
    return True
