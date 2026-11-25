#!/usr/bin/env python3
"""Cross-check reference encoder tables with LLVM's independent RDNA2 assembler."""
import argparse
from pathlib import Path
import re
import subprocess
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))
from reference.rdna2_scalar import FORMS, widths, encode


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--llvm-mc", default="llvm-mc")
    args = parser.parse_args()

    def reg(n, w):
        return f"s[{n}:{n+1}]" if w == 2 else f"s{n}"

    assembly, expected = [], []
    for name in sorted(FORMS):
        dw, aw, bw = widths(name)
        operands = ([reg(2,dw)] if dw else []) + [reg(4,aw)] + ([reg(6,bw)] if bw else [])
        assembly.append(name + " " + ", ".join(operands))
        expected.append(encode(name, 2 if dw else 0, 4, 6 if bw else 0))
    result = subprocess.run([args.llvm_mc, "-triple=amdgcn-amd-amdhsa",
                             "-mcpu=gfx1030", "-show-encoding"],
                            input="\n".join(assembly)+"\n", text=True,
                            capture_output=True, check=True, timeout=30)
    actual = [int.from_bytes(bytes(int(b,16) for b in row.split(",")), "little")
              for row in re.findall(r"encoding: \[([^\]]+)\]", result.stdout)]
    if actual != expected:
        raise ValueError("LLVM and reference encodings differ")
    print(f"LLVM verified {len(expected)} RDNA2 instruction encodings")


if __name__ == "__main__":
    main()
