#!/usr/bin/env python3
"""Check selected real Frosty global shaders with the checkout-local shader translator.

python tools/check_frosty_shaders.py /path/to/MA/Mount --output receipt.json

Reads the uncompressed Global archive member in the audited Frosty-XSX.pak.
Only the JSON receipt is retained. Requires liblz4, llvm-mc and DirectX Shader Compiler.
"""
import argparse
from collections import Counter
from concurrent.futures import ThreadPoolExecutor
from contextlib import closing
import ctypes
from ctypes.util import find_library
import hashlib
import json
from pathlib import Path
import re
import shutil
import struct
import subprocess
import sys
from tempfile import TemporaryDirectory
from threading import Lock
from uuid import UUID

import pefile

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from build import find_dxc
from check_frame_profile import decoded, interface, require, rip
from check_frosty_profile import BUILD, check_rows, symbols
from project_tuxbox.inspect import digest
from project_tuxbox.shader_prep import has_dxil_container, shader_compiler
from project_tuxbox.shader_inventory import ARCHIVES, STAGES, decoder, parse_tables, serialized_state

GLOBAL_SHA = "afab84fa891e491d64f1c2a9bdf26a66afc2054aef2cd9110e358402473b4fb2"
ENTRY = struct.Struct("<QIIB")  # offset in code area, stored bytes, decoded bytes, frequency
RAY_EXPORTS = {1280: "OcclusionMainRG_d7cd43b4", 1282: "DefaultPayloadMS_1b832205",
               1286: "IntersectionMainRG_d7cd43b4"}


def device_calls(mount):
    path = mount / "Frosty/Binaries/XSX/Frosty-XSX-Shipping.exe"
    require(digest(path) == BUILD, "Unknown Frosty shipping executable")
    llvm = shutil.which("llvm-objdump")
    require(llvm, "llvm-objdump is required")
    _, names = symbols(mount / "Symbols/Frosty-XSX-Shipping-Symbols.bin", path.name, {0x57aa048, 0x57661d7})
    device = UUID("728f1fa2-ea11-4a9f-9b52-9b42a484d5c9")
    state = UUID("47016943-fca8-4594-93ea-af258b55346d")
    properties = UUID("de5fa827-9bf9-4f26-89ff-d7f56fde3860")
    with closing(pefile.PE(str(path), fast_load=True)) as pe:
        base = pe.OPTIONAL_HEADER.ImageBase
        interface(pe, 0x57a9fcb, bytes.fromhex("488d15"), device)
        interface(pe, 0x57aa027, bytes.fromhex("488d05"), state)
        interface(pe, 0x57661cd, bytes.fromhex("488d15"), properties)
        message = pe.get_string_at_rva(rip(pe, 0x57aa072, bytes.fromhex("488d15")))
        require(message.startswith(b"Device8->DeserializeStateObjectX("), "Unexpected retail diagnostic")
        check_rows(decoded(llvm, path, base, 0x57a9fb7, 0x57aa04d), {
            0x57a9fd2: "call r9", 0x57aa016: "mov r10, qword ptr [rax + 0x398]",
            0x57aa03d: "mov r8d, dword ptr [rbp - 0x68]", 0x57aa048: "call r10"})
        check_rows(decoded(llvm, path, base, 0x57661c3, 0x5766261), {
            0x57661d7: "call r9", 0x57661ea: "mov r9, qword ptr [rax + 0x38]",
            0x57661f2: "xor edx, edx", 0x57661f4: "call r9", 0x5766211: "mov r9, qword ptr [rax + 0x38]",
            0x5766219: "lea edx, [r15 + 0x1]", 0x576621d: "call r9",
            0x5766233: "mov r8, qword ptr [rdx + 0x40]", 0x576623e: "call r8"})
    return {"title_sha256": BUILD, "method": "Device8->DeserializeStateObjectX", "device_iid": str(device),
            "title_caller": names[0x57aa048]["function"], "slot": 115, "call_rva": "0x57aa048",
            "output_iid": str(state), "inspection_function": names[0x57661d7]["function"],
            "properties_iid": str(properties), "properties_slot": 7, "properties_selectors": [0, 1],
            "state_object_inspection_slot": 8}


def global_archive(mount):
    with (mount / "Frosty/Content/Paks/Frosty-XSX.pak").open("rb") as pak:
        # Pak v11 directory entry 0x95c, data follows the 53-byte non-compressed file header.
        pak.seek(0x389806 + 53)
        blob = pak.read(0x14c3c50)
    require(hashlib.sha256(blob).hexdigest() == GLOBAL_SHA, "Global shader archive fingerprint differs")
    require(struct.unpack_from("<I", blob)[0] == 2, "Unexpected archive version")
    cursor = 4
    arrays = []
    for width in (20, 20, 16, ENTRY.size, 16, 4):
        count, = struct.unpack_from("<I", blob, cursor)
        cursor += 4
        arrays.append(memoryview(blob)[cursor:cursor + count * width])
        cursor += count * width
    entries = list(ENTRY.iter_unpack(arrays[3]))
    require(len(arrays[1]) == 20 * len(entries), "Shader hash count differs")
    require(max(offset + size for offset, size, _, _ in entries) == len(blob) - cursor,
            "Shader payload length differs")
    return memoryview(blob)[cursor:], entries


def decompress(payload, entry, lz4):
    offset, stored, length, _ = entry
    data = bytes(payload[offset:offset + stored])
    if stored == length:
        return data
    decoded = ctypes.create_string_buffer(length)
    written = lz4(data, decoded, stored, length)
    require(written == length, "LZ4 shader length differs")
    return decoded.raw


def containers(data):
    for match in re.finditer(b"DXBC", data):
        start = match.start()
        length, parts = struct.unpack_from("<II", data, start + 24)
        require(32 + 4 * parts <= length <= len(data) - start, "Invalid DXBC header")
        encoded = data[start:start + length]
        chunks = []
        for offset, in struct.iter_unpack("<I", encoded[32:32 + 4 * parts]):
            tag, size = struct.unpack_from("<4sI", encoded, offset)
            require(offset + 8 + size <= length, "Invalid DXBC chunk")
            chunks.append(tag.decode("ascii"))
        yield start, encoded, chunks


def translate(data, profile, work, xshader, dxc):
    ((_, encoded, _),) = containers(data)
    source, hlsl, portable = (work / name for name in ("shader.dxbc", "shader.hlsl", "shader.dxil"))
    source.write_bytes(encoded)
    run = subprocess.run([xshader, source, "--hlsl-out", hlsl], check=True, text=True, stdout=subprocess.PIPE)
    decode = re.search(r"decode: (\d+) instrs, consumed (\d+)/(\d+) bytes, terminated_on_s_endpgm=true", run.stdout)
    require(decode and decode[2] == decode[3], "Native shader decode did not cover the ISA")
    subprocess.run([dxc, "-T", profile, "-E", "main", "-Fo", portable, hlsl], check=True)
    ((_, _, chunks),) = containers(portable.read_bytes())
    require("DXIL" in chunks, "Expected DXIL from the desktop compiler")
    return {"instructions": int(decode[1]), "isa_bytes": int(decode[2]), "profile": profile,
            "dxil_bytes": portable.stat().st_size, "dxil_sha256": digest(portable)}


def survey_globals(payload, entries, lz4, xscan, work):
    bundle = work / "global-containers.bin"
    with bundle.open("wb") as out:
        for entry in entries:
            if entry[3] < 6:
                for _, encoded, _ in containers(decompress(payload, entry, lz4)):
                    out.write(encoded)
    scan = subprocess.run([xscan, bundle], text=True, capture_output=True, check=True)
    totals = re.search(r"containers found: (\d+)\s+unique: (\d+)\s+translated: (\d+)\s+failed: (\d+)", scan.stdout)
    require(totals, "Missing xscan translation summary")
    stages = {stage: {"translated": int(ok), "failed": int(fail)}
              for stage, ok, fail in re.findall(r"^\s+(\w+): ok=(\d+) fail=(\d+)$", scan.stdout, re.M)}
    found, unique, translated, failed = map(int, totals.groups())
    require(unique == translated + failed and translated == sum(row["translated"] for row in stages.values())
            and failed == sum(row["failed"] for row in stages.values()), "Inconsistent per-stage totals")
    return {"scope": "Non-ray global archive entries, deduplicated native containers translated to HLSL",
            "xscan_sha256": digest(xscan), "containers": found, "unique": unique,
            "translated": translated, "failed": failed, "stages": stages}


def state_programs(blob):
    previous_end = 0
    for pos, root, chunks in containers(blob):
        require(chunks == ["RTS0"] and pos >= previous_end, "Unexpected container in ray state")
        metadata = blob[previous_end:pos]
        cursor = pos + len(root)
        for expected_size in (28, 20):
            size, = struct.unpack_from("<I", blob, cursor)
            require(size == expected_size, "Unexpected program metadata length")
            cursor += 4 + size
        size, = struct.unpack_from("<I", blob, cursor)
        offset = cursor + 4
        code = blob[offset:offset + size]
        require(size and len(code) == size and size % 4 == 0, "Invalid native program length")
        previous_end = offset + size
        yield offset, code, metadata


def ray_state(data, llvm_mc):
    start, blob = serialized_state(data)
    hash64 = 0xcbf29ce484222325
    for byte in blob:
        hash64 = (hash64 ^ byte) * 0x100000001b3 & 0xffffffffffffffff

    programs = []
    for offset, code, _ in state_programs(blob):
        size = len(code)
        endpgm = [idx for idx in range(0, size, 4) if code[idx:idx + 4] == bytes.fromhex("000081bf")]
        require(len(endpgm) == 1, "Cannot locate a unique aligned native endpgm")
        encoded = " ".join(f"0x{x:02x}" for x in code[:endpgm[0] + 4])
        disassembly = subprocess.run([llvm_mc, "--disassemble", "--triple=amdgcn-amd-amdhsa", "--mcpu=gfx1030"],
                                     input=encoded, capture_output=True, text=True, check=True)
        ops = sorted(set(re.findall(r"^\s*(image_bvh(?:64)?_intersect_ray|s_(?:swap|set)pc_b64|s_endpgm)\b",
                                    disassembly.stdout, re.MULTILINE)))
        require("s_endpgm" in ops and "s_setpc_b64" in ops, "Unexpected native program control flow")
        programs.append({"offset": offset, "bytes": size, "sha256": hashlib.sha256(code).hexdigest(),
                         "gfx1030_opcodes_observed": ops, "decoder_warnings": disassembly.stderr.count("warning:")})
    require(len(programs) == 4, "Ray object program count differs")
    return {"version": 2, "offset_in_decoded_entry": start, "bytes": len(blob),
            "fnv1a64": f"{hash64:016x}", "sha256": hashlib.sha256(blob).hexdigest(), "programs": programs}


RAY_DXC_FLAGS = ["-T", "lib_6_3", "-O3"]


def ray_cache_directory(root, dxc):
    # The release package supplies the compiler and validator as shared
    # libraries. A change to either must invalidate existing library outputs.
    components = [dxc, dxc.parent.parent / "lib/libdxcompiler.so", dxc.parent.parent / "lib/libdxil.so",
                  dxc.parent / "dxcompiler.dll", dxc.parent / "dxil.dll"]
    key = hashlib.sha256(json.dumps([RAY_DXC_FLAGS, [(f.name, digest(f)) for f in components if f.is_file()]])
                         .encode()).hexdigest()
    cache = root / key
    cache.mkdir(parents=True, exist_ok=True)
    return cache


def compile_ray_state(state, work, xshader, dxc, compiled, lock, cache):
    with TemporaryDirectory(dir=cache or work) as temp:
        raw, hlsl, dxil = (Path(temp) / f"state.{ext}" for ext in ("sptr", "hlsl", "dxil"))
        raw.write_bytes(state)
        run = subprocess.run([xshader, raw, "--hlsl-out", hlsl], capture_output=True)
        if run.returncode:
            require(run.returncode == 1 and run.stderr.strip(),
                    f"Ray translator failed unexpectedly: {run.stderr.decode(errors='replace')}")
            return False
        key = digest(hlsl)
        with lock:
            if key in compiled:
                return True
            compiled.add(key)
        # A DXC failure aborts the entire census, including duplicate HLSL
        # entries which shared this compilation.
        cached = cache / f"{key}.dxil" if cache else None
        if cached and cached.is_file() and has_dxil_container(cached.read_bytes()):
            return True
        subprocess.run([dxc, *RAY_DXC_FLAGS, "-Fo", dxil, hlsl], check=True)
        if cached:
            require(has_dxil_container(dxil.read_bytes()), "Expected compiled ray library")
            dxil.replace(cached)
        return True


def ray_census(mount, work=None, xshader=None, dxc=None, jobs=1, cache=None):
    decode = decoder()
    archives = {}
    hashes = [Counter() for _ in range(4)]
    label = "XDXR_TraceRay".encode("utf-16-le")
    with (mount / "Frosty/Content/Paks/Frosty-XSX.pak").open("rb") as pak:
        for name, (offset, length, expected) in ARCHIVES.items():
            pak.seek(offset)
            data = memoryview(pak.read(length))
            require(hashlib.sha256(data).hexdigest() == expected, f"Unexpected {name} archive")
            _, _, entries, _, payload = parse_tables(data)
            counts = Counter()
            compiled = set()
            lock = Lock()
            accepted = 0

            def inspect(entry):
                _, state = serialized_state(decompress(payload, entry, decode))
                programs = list(state_programs(state))
                require(len(programs) == 4, "Unexpected ray state program count")
                for _, _, metadata in programs[:3]:
                    require(metadata.count(label) == 2, "Unrecognized traversal program label")
                codes = [hashlib.sha256(code).hexdigest() for _, code, _ in programs]
                ok = xshader and compile_ray_state(state, work, xshader, dxc, compiled, lock, cache)
                return STAGES[entry[3]], codes, bool(ok)

            ray_entries = [entry for entry in ENTRY.iter_unpack(entries) if entry[3] >= 6]
            with ThreadPoolExecutor(max_workers=jobs) as pool:
                for index, (stage, codes, ok) in enumerate(pool.map(inspect, ray_entries), 1):
                    for group, code in zip(hashes, codes):
                        group[code] += 1
                    counts[stage] += 1
                    accepted += ok
                    if xshader and (index % 500 == 0 or index == len(ray_entries)):
                        print(f"{name}: {index}/{len(ray_entries)} states, {accepted} compiled",
                              file=sys.stderr, flush=True)
            archives[name] = {"archive_sha256": expected, "entries": dict(counts)}
            if xshader:
                archives[name]["desktop_translation"] = {"compiled_entries": accepted,
                    "unsupported_entries": sum(counts.values()) - accepted, "distinct_libraries": len(compiled)}
    return {
        "archives": archives,
        "serialized_state_objects": sum(sum(a["entries"].values()) for a in archives.values()),
        "layout": "Version 2. Each of the four native programs follows a DXBC root signature containing only RTS0 and two size-prefixed metadata records. Each of the first three program headers contains the UTF-16 name XDXR_TraceRay twice.",
        "traversal_programs_by_position_and_sha256": [dict(group) for group in hashes[:3]],
        "distinct_fourth_programs": len(hashes[3]),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mount", type=Path)
    parser.add_argument("--ray-census-only", action="store_true", help="inspect serialized ray states in both archives")
    parser.add_argument("--compile-ray-census", action="store_true",
                        help="also run xshader and DXC over the full ray state census")
    parser.add_argument("--ray-cache", type=Path, help="reuse completed DXIL by exact HLSL and DXC build")
    parser.add_argument("--jobs", type=int, default=1, help="parallel workers for the ray state census")
    parser.add_argument("--xshader", type=Path)
    parser.add_argument("--xscan", type=Path, help="also survey HLSL translation across the non-ray global archive")
    parser.add_argument("--dxc", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--compat-log", type=Path, help="verify the retail DeserializeStateObjectX input against the archive")
    args = parser.parse_args()
    if args.ray_cache and not args.compile_ray_census:
        parser.error("--ray-cache requires --compile-ray-census")
    if args.jobs < 1:
        parser.error("--jobs must be positive")
    if args.ray_census_only or args.compile_ray_census:
        xshader = (args.xshader or shader_compiler()).resolve() if args.compile_ray_census else None
        dxc = Path(find_dxc(args.dxc)).resolve() if args.compile_ray_census else None
        with TemporaryDirectory() as temp:
            work = Path(temp)
            # A full archive scan can overlap a compiler rebuild.
            if xshader:
                xshader = Path(shutil.copy2(xshader, work / "xshader"))
            cache = ray_cache_directory(args.ray_cache, dxc) if args.ray_cache else None
            results = {"device_contract": device_calls(args.mount),
                       **ray_census(args.mount, work, xshader, dxc, args.jobs, cache)}
            if xshader:
                results["xshader_sha256"] = digest(xshader)
                results["dxc_sha256"] = digest(dxc)
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(results, indent=2) + "\n")
        return
    llvm_mc = shutil.which("llvm-mc")
    require(llvm_mc, "llvm-mc is required")
    xshader = args.xshader.resolve() if args.xshader else shader_compiler()
    dxc = Path(find_dxc(args.dxc)).resolve()
    payload, entries = global_archive(args.mount)
    lib = find_library("lz4")
    require(lib, "liblz4 is required")
    lz4 = ctypes.CDLL(lib).LZ4_decompress_safe
    lz4.argtypes = [ctypes.c_char_p, ctypes.c_void_p, ctypes.c_int, ctypes.c_int]
    result = {"global_archive_sha256": GLOBAL_SHA, "shader_count": len(entries),
              "device_contract": device_calls(args.mount),
              "counts_by_frequency": dict(sorted(Counter(row[3] for row in entries).items())),
              "xshader_sha256": digest(xshader), "dxc_sha256": digest(dxc), "samples": []}
    with TemporaryDirectory() as temp:
        if args.xscan:
            result["global_translation"] = survey_globals(payload, entries, lz4, args.xscan.resolve(), Path(temp))
        for index in (0, 1, *RAY_EXPORTS):
            entry = entries[index]
            data = decompress(payload, entry, lz4)
            row = {"index": index, "frequency": entry[3], "stored_bytes": entry[1],
                   "decoded_bytes": len(data), "decoded_sha256": hashlib.sha256(data).hexdigest()}
            if index in RAY_EXPORTS:
                name = RAY_EXPORTS[index]
                require(name.encode() + b"\0" in data[:128], "Unexpected ray entry point")
                row.update(export=name, container="SPTR", device_input=ray_state(data, llvm_mc))
            else:
                row["translation"] = translate(data, "vs_6_0" if index == 0 else "cs_6_0", Path(temp), xshader, dxc)
            result["samples"].append(row)
    if args.compat_log:
        log = args.compat_log.read_text(errors="replace")
        first = next(row for row in result["samples"] if row.get("index") == 1280)
        state = first["device_input"]
        call = (f'DeserializeStateObjectX bytes={state["bytes"]} fnv1a64={state["fnv1a64"]} '
                'prefix=[02, 00, 00, 00, 00, 00, 00, 00] root=true flags=0x0 '
                'iid={47016943-fca8-4594-93ea-af258b55346d} E_NOTIMPL')
        require(call in log, "Retail trace does not match OcclusionMainRG")
        result["retail_call"] = {"matched_archive_index": first["index"], "observed": call}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
