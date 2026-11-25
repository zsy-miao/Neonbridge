#!/usr/bin/env python3
r"""Recheck the Fortnite XSX device profile against the one audited x64 build.

From the project root:
  python tools/check_frame_profile.py "/path/to/FortniteClient-XSX-Shipping.exe" \
      --output evidence/fortnite-device.json

Requires pefile and llvm-objdump. Writes build fingerprints, relative addresses,
interface IDs and decoded record fields. Does not load or execute the image.
"""
import argparse
from contextlib import closing
import hashlib
import json
from pathlib import Path
import re
import struct
import subprocess
import sys
from uuid import UUID

import pefile


BUILD = "0b3d2e5968b0913eb01f52612793bfcc1984ff9764bd15d1c5aabf4830fcc3a0"
DEVICE = UUID("189819f1-1db6-4b57-be54-1821339b85f7")
QUEUE = UUID("0ec870a6-5d7e-4c22-8cfc-5baae07616ed")
LIST = UUID("5b160d0f-ac1b-4185-8ba8-b3ae42a5a455")
FACTORY = "D3D12XboxCreateDevice"


def require(condition, message):
    if not condition:
        raise ValueError(message)


def rip(pe, rva, prefix):
    instruction = pe.get_data(rva, len(prefix) + 4)
    require(instruction.startswith(prefix), f"Unexpected instruction at {rva:#x}")
    return rva + len(instruction) + struct.unpack_from("<i", instruction, len(prefix))[0]


def interface(pe, rva, prefix, iid):
    address = rip(pe, rva, prefix)
    require(pe.get_data(address, 16) == iid.bytes_le, f"Wrong interface at {address:#x}")
    return hex(address)


def decoded(executable, path, base, start, end):
    text = subprocess.run([executable, "-d", "-M", "intel", "--no-show-raw-insn",
                           f"--start-address={base+start}", f"--stop-address={base+end}", str(path)],
                          check=True, capture_output=True, text=True, timeout=60).stdout
    rows = {}
    for line in text.splitlines():
        match = re.match(r"^\s*([\da-fA-F]+):\s+(.+)$", line)
        if match:
            code = match[2].split("#", 1)[0].split(" <", 1)[0].strip()
            rows[int(match[1], 16) - base] = " ".join(code.split())
    return rows


def record(instructions):
    # Establish the definitions of the source registers used by stack stores.
    expected = {
        0x15ABDD6: "mov rdi, rcx", 0x15ABDD9: "mov r15d, 0x1", 0x15ABDDF: "xor r12d, r12d",
        0x15ABDEA: "mov ecx, dword ptr [rcx + 0x910]",
        0x15ABE5F: "vpxor xmm0, xmm0, xmm0", 0x15ABE6F: "mov eax, dword ptr [rdi + 0x910]",
        0x15ABE78: "or eax, -0x1", 0x15ABEAF: "mov ebx, r12d", 0x15ABEC3: "test eax, eax",
        0x15ABEC5: "cmovne ebx, r15d", 0x15ABED2: "mov rcx, r13", 0x15ABECC: "lea r13, [rdi + 0x18]",
        0x15ABEDA: "mov r9, r13", 0x15ABEE4: "lea rdx, [rbp - 0x80]", 0x15ABEE8: "xor ecx, ecx",
    }
    for rva, wanted in expected.items():
        require(instructions.get(rva) == wanted, f"Unexpected profile initializer at {rva:#x}")
    words = [None] * 16
    for rva, instruction in instructions.items():
        if not 0x15ABE53 <= rva <= 0x15ABEC9:
            continue
        store = re.fullmatch(r"(?:mov|vmovdqu) (dword|qword|xmmword) ptr \[rbp - (0x[\da-f]+)\], (.+)", instruction)
        if not store:
            continue
        word, source = (0x80 - int(store[2], 16)) // 4, store[3]
        count = {"dword": 1, "qword": 2, "xmmword": 4}[store[1]]
        if source in ("r12", "r12d", "xmm0"):
            value = 0
        elif source == "eax":
            value = "title field at +0x910 (also passed to D3D12XboxSetProcessDebugFlags)" if rva < 0x15ABE78 else 0xffffffff
        elif source == "ebx":
            value = "configuration boolean"
        else:
            value = int(source, 0)
        require(0 <= word and word + count <= len(words) and all(v is None for v in words[word:word+count]),
                f"Overlapping or out-of-range store at {rva:#x}")
        words[word:word+count] = [value] * count
    require(None not in words, "Incomplete factory profile")
    return words


def inspect(path, executable):
    with path.open("rb") as stream:
        digest = hashlib.file_digest(stream, "sha256").hexdigest()
    require(digest == BUILD, "Unknown executable hash. RVAs are scoped to the documented Fortnite XSX build.")
    with closing(pefile.PE(str(path), fast_load=True)) as pe:
        require(pe.FILE_HEADER.Machine == 0x8664, "Expected AMD64 PE")
        pe.parse_data_directories(directories=[pefile.DIRECTORY_ENTRY["IMAGE_DIRECTORY_ENTRY_IMPORT"]])
        base = pe.OPTIONAL_HEADER.ImageBase
        imports = [imp.address - base for mod in pe.DIRECTORY_ENTRY_IMPORT if mod.dll.lower() == b"d3d12_xs.dll"
                   for imp in mod.imports if imp.name == FACTORY.encode()]
        debug = "D3D12XboxSetProcessDebugFlags"
        debug_iat = [imp.address - base for mod in pe.DIRECTORY_ENTRY_IMPORT if mod.dll.lower() == b"d3d12_xs.dll"
                     for imp in mod.imports if imp.name == debug.encode()]
        require(debug_iat == [rip(pe, 0x15ABDF0, b"\xff\x15")], "Debug-flags call does not match its import")
        call = 0x15ABEEA
        require(imports == [rip(pe, call, b"\xff\x15")], "Factory call does not resolve to the expected import")
        factory_iid = interface(pe, 0x15ABEDD, b"\x4c\x8d\x05", DEVICE)
        config_rva = rip(pe, 0x15ABEA2, b"\x48\x8d\x15")
        config_key = "r.ForceStripAdjacencyDataDuringCooking"
        encoded = (config_key + "\0").encode("utf-16le")
        require(pe.get_data(config_rva, len(encoded)) == encoded, "Unexpected configuration key")
        calls = []
        for rva, opcode, iid_ref, iid_prefix, iid, slot, name in (
            (0xD2F275, b"\xff\x50\x40", 0xD2F261, b"\x4c\x8d\x05", QUEUE, 8, "ID3D12CommandQueue"),
            (0xD2F2E5, b"\xff\x90\x80\x01\x00\x00", 0xD2F2D0, b"\x4c\x8d\x05", QUEUE, 48, "ID3D12CommandQueue"),
            (0xE8C49A, b"\xff\x90\x48\x02\x00\x00", 0xE8C483, b"\x48\x8d\x15", LIST, 73, "ID3D12GraphicsCommandList"),
        ):
            require(pe.get_data(rva, len(opcode)) == opcode, f"Wrong vtable call at {rva:#x}")
            calls.append({"call_rva": hex(rva), "device_slot": slot, "requested_interface": name,
                          "iid_rva": interface(pe, iid_ref, iid_prefix, iid)})
        # Both queue calls live in one function. A type-2 branch repacks the
        # original descriptor before calling slot 48; treating it as a desktop
        # queue descriptor would swap the meaning of nonzero option words.
        queue = decoded(executable, path, base, 0xD2F23A, 0xD2F2EB)
        for rva, wanted in {
            0xD2F23A: "cmp dword ptr [r8], 0x2", 0xD2F241: "mov rsi, r8",
            0xD2F24A: f"je {base + 0xD2F29D:#x}", 0xD2F26B: "mov rdx, rsi",
            0xD2F29D: "mov eax, dword ptr [r8 + 0x8]", 0xD2F2A1: "mov dword ptr [rbp - 0x1c], eax",
            0xD2F2A4: "xor eax, eax", 0xD2F2A6: "cmp dword ptr [r8 + 0x4], eax",
            0xD2F2AA: "mov dword ptr [rbp - 0x20], 0x2", 0xD2F2B1: "setne al",
            0xD2F2B4: "and dword ptr [rbp - 0x14], 0x0", 0xD2F2BA: "mov dword ptr [rbp - 0x18], eax",
            0xD2F2DA: "lea rdx, [rbp - 0x20]",
        }.items():
            require(queue.get(rva) == wanted, f"Unexpected queue descriptor initializer at {rva:#x}")
        calls[0]["descriptor"] = "original descriptor when type != 2"
        calls[1]["descriptor_dwords"] = [2, "original dword 2", "original dword 1 != 0", 0]
        command_list = decoded(executable, path, base, 0xE8C3AF, 0xE8C573)
        for rva, wanted in {
            0xE8C3AF: "xor r8d, r8d", 0xE8C46D: "mov dword ptr [rbp - 0x20], eax",
            0xE8C470: "mov dword ptr [rbp - 0x18], edx", 0xE8C473: "mov dword ptr [rbp - 0x14], r8d",
            0xE8C477: "mov dword ptr [rbp - 0x1c], r8d", 0xE8C48F: "xor r9d, r9d",
            0xE8C496: "lea rdx, [rbp - 0x20]", 0xE8C4D6: f"call {base + 0xE8C524:#x}",
            0xE8C568: "mov rcx, qword ptr [rbx + 0x30]", 0xE8C56F: "call qword ptr [rax + 0x48]",
        }.items():
            require(command_list.get(rva) == wanted, f"Unexpected command list setup at {rva:#x}")
        calls[2]["creation_reserved_words"] = [0, 0]
        calls[2]["initial_pipeline"] = "null"
        calls[2]["initial_close"] = {"command_list_slot": 9, "call_rva": "0xe8c56f"}
        words = record(decoded(executable, path, base, 0x15ABD9C, call + 6))
        return {"kind": "static call sites", "executable_sha256": digest,
                "factory": {"import": f"d3d12_xs.dll!{FACTORY}", "iat_rva": hex(imports[0]), "call_rva": hex(call),
                            "adapter": "null", "requested_interface": "ID3D12Device", "iid_rva": factory_iid,
                            "record_dwords": words, "configuration_key_for_dword_9": config_key},
                "device_calls": calls}


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("executable", type=Path)
    parser.add_argument("--llvm-objdump", default="llvm-objdump")
    parser.add_argument("--output", type=Path, help="write JSON here after all checks pass (default: stdout)")
    args = parser.parse_args()
    try:
        report = json.dumps(inspect(args.executable, args.llvm_objdump), indent=2) + "\n"
        if args.output:
            args.output.write_text(report, encoding="utf-8")
        else:
            sys.stdout.write(report)
    except (OSError, ValueError, pefile.PEFormatError, subprocess.SubprocessError) as exc:
        parser.exit(1, f"Frame profile check failed: {exc}\n")


if __name__ == "__main__":
    main()
