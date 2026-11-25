#!/usr/bin/env python3
"""Map the audited Frosty build's GDK interface queries to its shipping symbols."""
import argparse
import csv
from contextlib import closing
from pathlib import Path
import re
import sys
from uuid import UUID

import pefile

from check_frame_profile import decoded, require, rip
from check_frosty_profile import BUILD, symbols

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from project_tuxbox.inspect import digest

# The statically linked GDK wrappers occupy this range in build 1.20.1831.3754.
QUERY = 0x5DAABF4
FIRST = 0x5DAAC4C
END = 0x5DAF400
GUARD_DISPATCH_IAT = 0x60D72E0


def guid(pe, rva, opcode):
    return str(UUID(bytes_le=pe.get_data(rip(pe, rva, opcode), 16)))


def describe(pe, instructions, site):
    registers = {}
    # Read only the straight-line argument setup feeding the helper.
    for rva, op in reversed(instructions[:site]):
        if op.startswith(("ret", "int3", "call ", "j")):
            break
        if op.startswith("lea rcx, [rip"):
            registers.setdefault("class", guid(pe, rva, b"\x48\x8d\x0d"))
        elif op.startswith("lea rdx, [rip"):
            registers.setdefault("iid", guid(pe, rva, b"\x48\x8d\x15"))
        elif op == "mov rdx, rcx":
            registers.setdefault("iid", "same")
    cls = registers.get("class")
    iid = registers.get("iid")
    require(cls and iid, f"Unrecognized query arguments at {instructions[site][0]:#x}")
    # The success branch loads the method from the interface's vtable and invokes
    # the CFG dispatch thunk. Stop at the first call so Release is never substituted.
    slot = None
    for rva, op in instructions[site + 1:]:
        method = re.fullmatch(r"mov rax, qword ptr \[rax \+ (0x[0-9a-f]+)\]", op)
        if method:
            offset = int(method[1], 16)
            require(offset % 8 == 0, f"Unaligned method offset at {rva:#x}")
            slot = offset // 8
        if op.startswith("call "):
            require(slot is not None and op.startswith("call qword ptr [rip")
                    and rip(pe, rva, b"\xff\x15") == GUARD_DISPATCH_IAT,
                    f"Unrecognized method call at {rva:#x}")
            return cls, cls if iid == "same" else iid, slot
        require(op != "ret", f"Missing interface call at {rva:#x}")
    raise ValueError("Unexpected end of wrapper range")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mount", type=Path)
    args = parser.parse_args()
    exe = args.mount / "Frosty/Binaries/XSX/Frosty-XSX-Shipping.exe"
    require(digest(exe) == BUILD, "Unknown Frosty executable")
    with closing(pefile.PE(str(exe), fast_load=True)) as pe:
        base = pe.OPTIONAL_HEADER.ImageBase
        rows = list(decoded("llvm-objdump", exe, base, FIRST, END).items())
        sites = {rva: i for i, (rva, op) in enumerate(rows) if op == f"call {base + QUERY:#x}"}
        require(len(sites) == 140, "Unexpected number of direct interface queries")
        _, names = symbols(args.mount / "Symbols/Frosty-XSX-Shipping-Symbols.bin", exe.name, sites)
        writer = csv.writer(sys.stdout, lineterminator="\n")
        writer.writerow(["wrapper", "wrapper_rva", "query_call_rva", "class_guid", "iid", "slot"])
        for rva, index in sites.items():
            name = names[rva]
            writer.writerow([name["function"], name["function_rva"], hex(rva), *describe(pe, rows, index)])


if __name__ == "__main__":
    main()
