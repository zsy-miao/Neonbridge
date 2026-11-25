#!/usr/bin/env python3
r"""Verify graphics call sites and their symbols in the audited Frosty shipping build.

  python tools/check_frosty_profile.py "/path/to/MA/Mount" --output evidence/frosty-device.json

Reads the manifest, shipping PE and its matching Symbols file. Requires pefile
and llvm-objdump. Emits static facts, without paths or binary content.
"""
import argparse
from contextlib import closing
import json
from pathlib import Path, PureWindowsPath
import re
import struct
import subprocess
import sys
from uuid import UUID
from xml.etree import ElementTree

import pefile

from check_frame_profile import DEVICE, QUEUE, LIST, decoded, interface, require, rip

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from project_tuxbox.inspect import digest
from project_tuxbox.symbols import lookup_symbols


BUILD = "ae39eb1e92fa806df189c768c1478a53dfb54a2cc9769a9f2301ca7921e3eb04"
SYMBOLS = "b886b339a77da68874a44d107e366aeb4b7c16695dd67e8bbd44d6fcbab13c2c"

DXC_UTILS_CLASS = UUID("6245d6af-66e0-48fd-80b4-4d271796748c")
DXC_UTILS_IID = UUID("4605c4cb-2019-492a-ada4-65f20bb7d67f")
SIGNATURE_IID = UUID("c36a797c-ec80-4f0a-8985-a7b2475082d1")
# (call, stride store, IID reference, null root, diagnostic, name, argument type, stride)
STANDARD_SIGNATURES = (
    (0x57682EF, 0x57682A0, 0x57682D2, 0x57682D9, 0x576831B, "DrawIndirect", 0, 16),
    (0x576837E, 0x5768338, 0x5768361, 0x5768368, 0x57683AA, "DrawIndexedIndirect", 1, 20),
    (0x576840D, 0x57683C7, 0x57683F0, 0x57683F7, 0x5768439, "DispatchIndirectGraphics", 2, 12),
)
INDIRECT_COMPUTE_CALL = 0x57876DF
NULL_VIEW_CALLS = (0x5790412, 0x57904A8, 0x579054F)
INITIAL_CLOSE = 0x5786909
CREATE_HEAP, PLACE_RESOURCE = 0x57A419E, 0x57C54EC
CREATE_DISPLAY = 0x57642C5
VOLUME_ALLOCATION = 0x57612D4
TEXTURE_ALLOCATOR = 0x577D610
DXC_FACTORIES = (0x5767C54, 0x5767CAE)
# (GDxcUtils load, input pointer -> RDX, input pointer -> RBX, CreateBlob call)
DXC_BLOB_CALLS = ((0x5753EFB, 0x5753F0D, 0x5753F10, 0x5753F16),
                  (0x57545F1, 0x5754603, 0x5754606, 0x575460C),
                  (0x5754879, 0x5754888, 0x575488B, 0x5754896))

# Factory, get_layout, plane copy stride, copy size, release.
XG_CALLS = ((0x575CCFA, 0x575CD45, 0x575CDF8, 0x575CE09, 0x575CE28),
            (0x575ED9B, 0x575EDE5, 0x575EE9D, 0x575EEB1, 0x575EED0))


def check_rows(instructions, expected):
    for rva, wanted in expected.items():
        require(instructions.get(rva) == wanted, f"Unexpected profile instruction at {rva:#x}")


def symbols(path, executable_name, sites):
    require(digest(path) == SYMBOLS, "Unknown shipping symbol file")
    data = lookup_symbols(path, sites)
    require(data["module"] == executable_name, "Symbol image mismatch")
    covered = {}
    for row in data["lookups"]:
        require(row["status"] != "ambiguous", "Ambiguous symbol range")
        require(row["status"] == "matched", "Missing symbol for a call site")
        covered[int(row["rva"], 16)] = {"function": row["function"], "function_rva": row["function_rva"]}
    return data["symbol_count"], covered


def factory_record(rows):
    check_rows(rows, {
        0x5752AAB: "mov r15, rcx", 0x5752AB2: "mov dword ptr [rcx + 0x980], 0x1",
        0x5752ABC: "mov ecx, 0x1", 0x5752AC3: "mov ecx, dword ptr [rcx + 0x980]", 0x5752ACF: "xor esi, esi",
        0x5752B68: "mov eax, dword ptr [r15 + 0x980]", 0x5752B88: "mov eax, esi", 0x5752B91: "setne al",
        0x5752BB4: "lea r9, [r15 + 0x18]", 0x5752BBF: "lea rdx, [rbp - 0x80]", 0x5752BC3: "xor ecx, ecx",
    })
    require(rows.get(0x5752B8A, "").startswith("cmp byte ptr [rip + ") and rows[0x5752B8A].endswith("], sil"),
            "Expected the boolean field to be loaded from a global byte")
    fields = [None] * 18
    # The caller initializes the 72 bytes beginning at its RDX argument.
    # Obtain each offset, width and value from LLVM's decoded stores.
    for rva, row in rows.items():
        if not 0x5752B4F <= rva <= 0x5752B97:
            continue
        match = re.fullmatch(r"mov (dword|qword) ptr \[rbp - (0x[0-9a-f]+)\], (.+)", row)
        if not match:
            continue
        first, count = (0x80 - int(match[2], 16)) // 4, 1 if match[1] == "dword" else 2
        source = match[3]
        if source == "eax":
            value = "title field at +0x980 (also passed to D3D12XboxSetProcessDebugFlags)" if rva == 0x5752B6F else "global byte != 0"
        elif source in ("esi", "rsi"):
            value = 0
        else:
            value = int(source, 0) & 0xffffffff
        require(0 <= first and first + count <= len(fields) and all(v is None for v in fields[first:first+count]),
                f"Invalid record store at {rva:#x}")
        fields[first:first+count] = [value] * count
    require(None not in fields, "Incomplete device record")
    return fields


def dxc_usage(pe, llvm, path, context, factory_iat):
    global_rva = rip(pe, DXC_FACTORIES[0] - 21, b"\x4c\x8d\x05")
    for call in DXC_FACTORIES:
        # LEA R8 (output), RDX (IID), RCX (CLSID), then CALL [import].
        require(rip(pe, call - 21, b"\x4c\x8d\x05") == global_rva, "Different DXC output globals")
        interface(pe, call - 14, b"\x48\x8d\x15", DXC_UTILS_IID)
        interface(pe, call - 7, b"\x48\x8d\x0d", DXC_UTILS_CLASS)
        require(rip(pe, call, b"\xff\x15") == factory_iat, "Wrong DXC factory import")
    free_iat, = (imp.address - pe.OPTIONAL_HEADER.ImageBase for mod in pe.DIRECTORY_ENTRY_IMPORT
                 if mod.dll.lower() == b"api-ms-win-crt-heap-l1-1-0.dll" for imp in mod.imports if imp.name == b"free")
    calls = []
    for load, data, saved, call in DXC_BLOB_CALLS:
        require(rip(pe, load, b"\x48\x8b\x0d") == global_rva, "Wrong IDxcUtils receiver")
        rows = decoded(llvm, path, pe.OPTIONAL_HEADER.ImageBase, load, call + 13)
        check_rows(rows, {load + 7: "xor r9d, r9d", data: "mov rdx, rax", saved: "mov rbx, rax",
                          call - 3: "mov r10, qword ptr [rcx]", call: "call qword ptr [r10 + 0x30]",
                          call + 4: "mov rcx, rbx"})
        require(rip(pe, call + 7, b"\xff\x15") == free_iat, "Expected caller to free the source after CreateBlob")
        calls.append({"function": context[call]["function"], "call_rva": hex(call)})
    return {"import": "dxcompiler_xs.dll!DxcCreateInstance", "class": "CLSID_DxcUtils", "clsid": str(DXC_UTILS_CLASS),
            "interface": "IDxcUtils", "iid": str(DXC_UTILS_IID),
            "factory_calls": [{"call_rva": hex(call), "function": context[call]["function"]} for call in DXC_FACTORIES],
            "method": "IDxcUtils::CreateBlob", "slot": 6, "code_page": 0,
            "source_freed_after_call": True, "blob_calls": calls}


def xg_usage(pe, llvm, path, context, factory_iat):
    calls = []
    for create, get_layout, stride, size, release in XG_CALLS:
        require(rip(pe, create, b"\xff\x15") == factory_iat, "Wrong XG factory import")
        rows = decoded(llvm, path, pe.OPTIONAL_HEADER.ImageBase, create, release + 3)
        check_rows(rows, {get_layout: "call qword ptr [rax + 0x10]", stride: "imul rdx, rax, 0x6c8",
                          size: "mov r8d, 0x6c8", release: "call qword ptr [rax + 0x8]"})
        calls.append({"function": context[create]["function"], "factory_rva": hex(create),
                      "get_layout_rva": hex(get_layout), "release_rva": hex(release)})
    return {"import": "xg_xs.dll!XGCreateTextureComputer", "get_layout_slot": 2, "release_slot": 1,
            "copied_bytes_per_plane": 0x6c8, "calls": calls}


def command_signatures(pe, llvm, path):
    rows = decoded(llvm, path, pe.OPTIONAL_HEADER.ImageBase, 0x5753180, 0x57533D9)
    signatures = []
    for call, slot_load, type_store, stride_store, log_ref, purpose, arg_type, stride in (
        (0x5753222, 0x5753205, 0x57531C4, 0x57531B4, 0x575324C, "DispatchIndirectCompute", 105, 12),
        (0x57532B7, 0x575329A, 0x575325A, 0x5753261, 0x57532E1, "DispatchIndirectMesh", 107, 16),
        (0x575339A, 0x575337D, 0x575331B, 0x575330B, 0x57533C4, "DispatchRaysIndirect", 9, 104),
    ):
        check_rows(rows, {call: "call r10", slot_load: "mov r10, qword ptr [rax + 0x148]",
                          type_store: f"mov dword ptr [rbp - 0x30], {arg_type:#x}",
                          stride_store: f"mov dword ptr [rbp - 0x20], {stride:#x}"})
        diagnostic = pe.get_string_at_rva(rip(pe, log_ref, b"\x48\x8d\x15"))
        require((purpose + "CommandSignature").encode() in diagnostic, "Unexpected command signature diagnostic")
        signatures.append({"purpose": purpose, "call_rva": hex(call), "device_slot": 41,
                           "argument_type": arg_type, "byte_stride": stride})
    return signatures


def standard_command_signatures(pe, llvm, path, context):
    rows = decoded(llvm, path, pe.OPTIONAL_HEADER.ImageBase, 0x5768260, 0x576845A)
    # The first draw argument is initialized by zeroing its entire 16-byte descriptor.
    check_rows(rows, {0x576828E: "mov dword ptr [rbp - 0x1c], 0x1",
                      0x5768298: "lea rax, [rbp - 0x30]", 0x576829C: "mov qword ptr [rbp - 0x18], rax",
                      0x57682AA: "vpxor xmm0, xmm0, xmm0", 0x57682AE: "vmovups xmmword ptr [rbp - 0x30], xmm0",
                      0x576832E: "mov dword ptr [rbp - 0x30], 0x1", 0x57683BD: "mov dword ptr [rbp - 0x30], 0x2"})
    result = []
    for call, size_store, iid_ref, root, log_ref, name, kind, stride in STANDARD_SIGNATURES:
        check_rows(rows, {call: "call r10", call - 7: "mov r10, qword ptr [rax + 0x148]",
                          size_store: f"mov dword ptr [rbp - 0x20], {stride:#x}", root: "xor r8d, r8d"})
        interface(pe, iid_ref, b"\x4c\x8d\x0d", SIGNATURE_IID)
        diagnostic = pe.get_string_at_rva(rip(pe, log_ref, b"\x48\x8d\x15"))
        require((name + "CommandSignature").encode() in diagnostic, "Unexpected standard signature diagnostic")
        result.append({**context[call], "purpose": name, "call_rva": hex(call), "device_slot": 41,
                       "argument_type": kind, "byte_stride": stride})
    return result


def indirect_dispatch(pe, llvm, path, context):
    rows = decoded(llvm, path, pe.OPTIONAL_HEADER.ImageBase, 0x5787380, INDIRECT_COMPUTE_CALL + 7)
    check_rows(rows, {0x578739D: "xor ebx, ebx", 0x57876AE: "mov r10, qword ptr [rcx]",
                      0x57876B8: "add rax, qword ptr [rsi + 0x100]", 0x57876C6: "mov qword ptr [rsp + 0x30], rbx",
                      0x57876CB: "mov qword ptr [rsp + 0x28], rbx", 0x57876D0: "mov qword ptr [rsp + 0x20], rax",
                      0x57876D5: "mov r9, qword ptr [r9 + 0x20]", 0x57876D9: "mov r8d, 0x1",
                      INDIRECT_COMPUTE_CALL: "call qword ptr [r10 + 0x1d8]"})
    return {**context[INDIRECT_COMPUTE_CALL], "call_rva": hex(INDIRECT_COMPUTE_CALL), "command_list_slot": 59,
            "max_command_count": 1, "count_buffer": "null", "count_buffer_offset": 0}


def null_views(pe, llvm, path, context):
    rows = decoded(llvm, path, pe.OPTIONAL_HEADER.ImageBase, 0x5790337, NULL_VIEW_CALLS[-1] + 3)
    check_rows(rows, {0x5790342: "mov r15d, 0x1", 0x579036E: "mov qword ptr [rbp - 0x51], 0x1688",
                      0x5790376: "mov qword ptr [rbp - 0x45], r15",
                      0x579037E: "mov dword ptr [rbp - 0x55], 0x4", 0x5790385: "mov dword ptr [rbp - 0x59], 0x57",
                      0x5790401: "mov r10, qword ptr [rax + 0x90]", 0x579040C: "lea r8, [rbp - 0x59]",
                      0x579041D: "mov dword ptr [rbp - 0x6d], 0x4", 0x5790424: "mov dword ptr [rbp - 0x71], 0x57",
                      0x5790497: "mov r10, qword ptr [rax + 0xa0]", 0x57904A2: "lea r8, [rbp - 0x71]",
                      0x57904BC: "mov dword ptr [rbp - 0x2d], 0x4", 0x57904C3: "mov dword ptr [rbp - 0x31], 0x1c",
                      0x579053A: "mov r10, qword ptr [rdx + 0x98]", 0x5790546: "lea r9, [rbp - 0x31]",
                      0x579054A: "xor r8d, r8d"})
    result = []
    for call, slot, method, fmt in zip(NULL_VIEW_CALLS, (18, 20, 19), ("SRV", "RTV", "UAV"),
                                        ("B8G8R8A8_UNORM", "B8G8R8A8_UNORM", "R8G8B8A8_UNORM"), strict=True):
        check_rows(rows, {call - 2: "xor edx, edx", call: "call r10"})
        result.append({**context[call], "call_rva": hex(call), "device_slot": slot, "view": method,
                       "dimension": "TEXTURE2D", "format": fmt, "resource": "null"})
    return result


def inspect(mount, llvm):
    root = ElementTree.parse(mount / "MicrosoftGame.config")
    decl, = root.findall("./{*}ExecutableList/{*}Executable")
    require(decl.get("TargetDeviceFamily") == "Scarlett", "Expected a Scarlett title")
    relative = PureWindowsPath(decl.attrib["Name"])
    require(relative.as_posix() == "Frosty/Binaries/XSX/Frosty-XSX-Shipping.exe", "Unexpected executable declaration")
    path = mount / relative.as_posix()
    require(digest(path) == BUILD, "Unsupported Frosty executable hash")
    sites = [0x5752BC5, 0x5766CAB, 0x575291D, 0x575284A, 0x5786667, 0x5751C9B]
    symbol_sites = [*sites, *DXC_FACTORIES, *(row[3] for row in DXC_BLOB_CALLS), *(row[0] for row in XG_CALLS),
                    *(row[0] for row in STANDARD_SIGNATURES), INDIRECT_COMPUTE_CALL, *NULL_VIEW_CALLS, INITIAL_CLOSE,
                    CREATE_HEAP, PLACE_RESOURCE, CREATE_DISPLAY, VOLUME_ALLOCATION, TEXTURE_ALLOCATOR]
    count, context = symbols(mount / "Symbols/Frosty-XSX-Shipping-Symbols.bin", path.name, symbol_sites)
    with closing(pefile.PE(str(path), fast_load=True)) as pe:
        require(pe.FILE_HEADER.Machine == 0x8664, "Expected AMD64 PE")
        base = pe.OPTIONAL_HEADER.ImageBase
        pe.parse_data_directories(directories=[pefile.DIRECTORY_ENTRY["IMAGE_DIRECTORY_ENTRY_IMPORT"]])
        graphics = {mod.dll.decode().lower(): {imp.name.decode(): imp.address - base for imp in mod.imports}
                    for mod in pe.DIRECTORY_ENTRY_IMPORT if mod.dll.lower() in (b"d3d12_xs.dll", b"xg_xs.dll", b"dxcompiler_xs.dll")}
        for site, export in ((sites[0], "D3D12XboxCreateDevice"), (sites[1], "D3D12CreateDevice"),
                             (0x5752AC9, "D3D12XboxSetProcessDebugFlags")):
            require(graphics["d3d12_xs.dll"][export] == rip(pe, site, b"\xff\x15"), f"Wrong import for {site:#x}")
        factory = {**context[sites[0]], "call_rva": hex(sites[0]), "adapter": "null",
                   "requested_interface": "ID3D12Device", "iid_rva": interface(pe, 0x5752BB8, b"\x4c\x8d\x05", DEVICE),
                   "record_dwords": factory_record(decoded(llvm, path, base, 0x5752A70, 0x5752BCB))}
        # Resolve the read of dword 15 back to its producer. The writes have a
        # trailing 1-byte immediate after disp32, so next RIP is one byte later.
        flag = rip(pe, 0x5752B8A, b"\x40\x38\x35")
        for store, value in ((0x5751C9B, 1), (0x5751CC3, 0)):
            require(rip(pe, store, b"\xc6\x05") + 1 == flag and pe.get_data(store + 6, 1) == bytes([value]),
                    "Global factory flag does not match the RHI initialization writes")
        require(rip(pe, 0x5751CB9, b"\xe8") == 0x5771AF0, "Unexpected log target on the flag-set branch")
        message = rip(pe, 0x5771AF4, b"\x48\x8d\x05")
        expected = "AutoHDR output is enabled.\0".encode("utf-16le")
        require(pe.get_data(message, len(expected)) == expected, "Unexpected diagnostic for the flag-set branch")
        factory["dword_15_source"] = {**context[sites[5]], "flag_rva": hex(flag),
                                      "true_branch_log": "AutoHDR enabled", "log_call_rva": hex(0x5751CB9)}
        calls = []
        for site, load, slot, ref, prefix, iid, name in (
            (sites[2], 0x5752919, 8, 0x5752909, b"\x4c\x8d\x05", QUEUE, "ID3D12CommandQueue"),
            (sites[3], 0x5752843, 48, 0x5752831, b"\x4c\x8d\x05", QUEUE, "ID3D12CommandQueue"),
            (sites[4], 0x5786639, 73, 0x5786645, b"\x48\x8d\x05", LIST, "ID3D12GraphicsCommandList"),
        ):
            rows = decoded(llvm, path, base, load, site + 3)
            check_rows(rows, {load: f"mov r10, qword ptr [rax + {slot*8:#x}]", site: "call r10"})
            calls.append({**context[site], "call_rva": hex(site), "device_slot": slot, "requested_interface": name,
                          "iid_rva": interface(pe, ref, prefix, iid)})
        rows = decoded(llvm, path, base, 0x578660E, INITIAL_CLOSE + 4)
        check_rows(rows, {0x578660E: "xor r8d, r8d", 0x5786617: "mov dword ptr [rsp + 0x88], eax",
                          0x578661E: "mov dword ptr [rsp + 0x90], edx", 0x5786625: "mov dword ptr [rsp + 0x94], r8d",
                          0x578662D: "mov dword ptr [rsp + 0x8c], r8d", 0x5786651: "xor r9d, r9d",
                          0x578665C: "lea rdx, [rsp + 0x88]", 0x57868FF: "mov rcx, qword ptr [rsi]",
                          0x5786905: "mov rdx, qword ptr [rax + 0x48]", INITIAL_CLOSE: "call rdx"})
        heap_iid = interface(pe, 0x57A4154, b"\x4c\x8d\x05", UUID("6b3b2502-6e51-45b3-90ee-9884265e8df3"))
        resource_iid = interface(pe, 0x57C54CD, b"\x48\x8d\x05", UUID("696442be-a72e-4059-bc79-5b5c98040fad"))
        check_rows(decoded(llvm, path, base, 0x57A4181, CREATE_HEAP + 3),
                   {0x57A4181: "lea rdx, [rbp + 0x10]", 0x57A4197: "mov r10, qword ptr [rax + 0xe0]",
                    CREATE_HEAP: "call r10"})
        check_rows(decoded(llvm, path, base, 0x57C54BC, PLACE_RESOURCE + 3),
                   {0x57C54BC: "mov r10, qword ptr [rax + 0xe8]", 0x57C54DE: "mov dword ptr [rsp + 0x20], r12d",
                    0x57C54E3: "mov r9, rdi", 0x57C54E6: "mov r8, r11", 0x57C54E9: "mov rdx, r14",
                    PLACE_RESOURCE: "call r10"})
        check_rows(decoded(llvm, path, base, 0x5764176, CREATE_DISPLAY + 3),
                   {0x5764176: "mov r11d, 0x1", 0x576417C: "mov qword ptr [rbp + 0x30], r11",
                    0x57641EF: "mov dword ptr [rbp - 0x80], 0x3", 0x5764202: "mov dword ptr [rbp - 0x64], 0x10001",
                    0x5764209: "mov dword ptr [rbp - 0x60], edx", 0x5764214: "mov dword ptr [rbp - 0x50], r11d",
                    0x5764294: "mov r10, qword ptr [rax + 0xd8]", 0x57642B8: "mov r8d, 0x8",
                    0x57642BE: "lea rdx, [rbp + 0x30]", CREATE_DISPLAY: "call r10"})
        require(rip(pe, VOLUME_ALLOCATION, b"\xe8") == TEXTURE_ALLOCATOR, "Unexpected volume allocation target")
        diagnostic = pe.get_string_at_rva(rip(pe, 0x5761300, b"\x48\x8d\x15"))
        require(b"AllocateTexture(TextureDesc, ClearValuePtr, Format, Texture3D->ResourceLocation" in diagnostic,
                "Unexpected volume allocation diagnostic")
        return {"kind": "static call sites", "package_version": root.find("./{*}Identity").attrib["Version"],
                "executable_sha256": BUILD, "shipping_symbols_sha256": SYMBOLS, "shipping_function_records": count,
                "graphics_imports": {mod: sorted(table) for mod, table in graphics.items()}, "xbox_factory": factory,
                "other_factory": {**context[sites[1]], "call_rva": hex(sites[1]), "import": "d3d12_xs.dll!D3D12CreateDevice"},
                "device_calls": calls,
                "command_signatures": command_signatures(pe, llvm, path),
                "dxc_utils": dxc_usage(pe, llvm, path, context, graphics["dxcompiler_xs.dll"]["DxcCreateInstance"]),
                "xg_texture": xg_usage(pe, llvm, path, context, graphics["xg_xs.dll"]["XGCreateTextureComputer"]),
                "standard_command_signatures": standard_command_signatures(pe, llvm, path, context),
                "indirect_compute_dispatch": indirect_dispatch(pe, llvm, path, context),
                "null_views": null_views(pe, llvm, path, context),
                "command_list_initial_close": {**context[INITIAL_CLOSE], "call_rva": hex(INITIAL_CLOSE),
                                               "command_list_slot": 9, "creation_reserved_words": [0, 0],
                                               "initial_pipeline": "null"},
                "memory_heap_calls": [
                    {**context[CREATE_HEAP], "call_rva": hex(CREATE_HEAP), "device_slot": 28,
                     "requested_interface": "ID3D12Heap", "iid_rva": heap_iid},
                    {**context[PLACE_RESOURCE], "call_rva": hex(PLACE_RESOURCE), "device_slot": 29,
                     "requested_interface": "ID3D12Resource", "iid_rva": resource_iid}],
                "display_allocation": {**context[CREATE_DISPLAY], "call_rva": hex(CREATE_DISPLAY), "device_slot": 27,
                                       "heap_type": "DEFAULT", "heap_flags": "ALLOW_DISPLAY",
                                       "dimension": "TEXTURE2D", "format": "selected by viewport"},
                "volume_allocation": {**context[VOLUME_ALLOCATION], "call_rva": hex(VOLUME_ALLOCATION),
                                      "target": context[TEXTURE_ALLOCATOR]["function"]}}


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("mount", type=Path)
    parser.add_argument("--llvm-objdump", default="llvm-objdump")
    parser.add_argument("--output", type=Path, help="write JSON after all checks pass (default: stdout)")
    args = parser.parse_args()
    try:
        result = json.dumps(inspect(args.mount, args.llvm_objdump), indent=2) + "\n"
        if args.output:
            args.output.write_text(result, encoding="utf-8")
        else:
            sys.stdout.write(result)
    except (OSError, ValueError, KeyError, ElementTree.ParseError, pefile.PEFormatError, subprocess.SubprocessError) as exc:
        parser.exit(1, f"Frosty profile check failed: {exc}\n")


if __name__ == "__main__":
    main()
