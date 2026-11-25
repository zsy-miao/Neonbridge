#!/usr/bin/env python3
"""Generate deterministic native scalar programs and independent CPU results."""
import argparse
from collections import Counter
import hashlib
import json
from pathlib import Path
import random
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))
from reference.rdna2_scalar import (END, EXTENDED, FORMS, MASK, REGS, decode,
                                   encode, execute, valid_reg, widths)

BOUNDARIES = (0, 1, 2, 7, 8, 15, 16, 31, 32, 33, 63, 64, 65, 127, 128, 255,
              256, 0x7fff, 0x8000, 0xffff, 0x10000, 0x7ffffffe, 0x7fffffff,
              0x80000000, 0x80000001, 0xfffffffe, MASK, 0xaaaaaaaa, 0x55555555)


def initialize(values, scc):
    words = []
    for r, value in zip(REGS, values):
        words.extend((encode("s_mov_b32", r, 255), value))
    # Compare inline constants so the input flag is independent of registers.
    words.append(encode("s_cmp_eq_u32", 0, 128, 128 if scc else 129))
    return words


def registers(width):
    return [r for r in REGS if valid_reg(r, width)]


def operation(name, rng, alias):
    dw, aw, bw = widths(name)
    a = rng.choice(registers(aw))
    b = rng.choice(registers(bw)) if bw else 0
    dest = rng.choice(registers(dw)) if dw else 0
    if dw and alias == 1 and valid_reg(a, dw):
        dest = a
    if dw and bw and alias == 2 and valid_reg(b, dw):
        dest = b
    return encode(name, dest, a, b), (dest, a, b)


def generate(seed, per_op, chains, profile):
    rng = random.Random(seed)
    names = sorted(name for name in FORMS if profile == "extended" or name not in EXTENDED)
    index = 0
    for name in names:
        dw, aw, bw = widths(name)
        for case in range(per_op):
            word, (dest, a, b) = operation(name, rng, case % 3)
            values = [rng.getrandbits(32) for _ in REGS]
            if case < len(BOUNDARIES):
                # Distinct operands where possible; aliasing itself is tested.
                values[a-2] = BOUNDARIES[case]
                if aw == 2:
                    values[a-1] = BOUNDARIES[-1-case]
                if bw:
                    values[b-2] = BOUNDARIES[(case*7+1)%len(BOUNDARIES)]
                    if bw == 2:
                        values[b-1] = BOUNDARIES[(case*11+3)%len(BOUNDARIES)]
            elif case < 2*len(BOUNDARIES):
                # Whole-word zero/ones, carry, signed limits, equal operands,
                # and mask/shift boundaries in both halves.
                values = [BOUNDARIES[case-len(BOUNDARIES)]]*len(REGS)
            prefix = initialize(values, case % 2)
            words = prefix+[word, END]
            yield f"{index:06d}", name, words
            index += 1
    for case in range(chains):
        values = [rng.getrandbits(32) for _ in REGS]
        words = initialize(values, case % 2)
        for j in range(8 + case % 9):
            name = rng.choice(names)
            word, _ = operation(name, rng, j%3)
            words.append(word)
        yield f"{index:06d}", "chain", words+[END]
        index += 1
    # Explicit literal/inline operands and carry propagation, not only SGPRs.
    words = initialize([MASK,0,1,0,0,0,0,0], 0)
    words += [encode("s_add_u32",2,2,129), encode("s_addc_u32",3,3,128),
              encode("s_sub_u32",4,4,255), 0x80000001,
              encode("s_subb_u32",5,5,128), encode("s_and_b32",6,6,193),
              encode("s_mov_b32",7,208), END]
    yield f"{index:06d}", "selectors_and_carry", words


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path)
    parser.add_argument("--seed", type=int, default=0x53434152)
    parser.add_argument("--per-op", type=int, default=64)
    parser.add_argument("--chains", type=int, default=128)
    parser.add_argument("--profile", choices=("core","extended"), default="core")
    args = parser.parse_args()
    if not 1 <= args.per_op <= 1024 or not 0 <= args.chains <= 1024:
        parser.error("per-op 1..1024, chains 0..1024")
    args.directory.mkdir(parents=True, exist_ok=False)
    counts = Counter()
    rows = []
    with (args.directory/"programs.txt").open("x") as f:
        for ident, category, words in generate(args.seed,args.per_op,args.chains,args.profile):
            counts[category] += 1
            expected = execute(words)
            f.write(ident+" "+" ".join(f"{word:08x}" for word in words)+"\n")
            rows.append({"id":ident,"category":category,"expected":expected})
    data = {"schema":1,"seed":args.seed,"per_op":args.per_op,"chains":args.chains,
            "profile":args.profile,"program_sha256":hashlib.sha256((args.directory/"programs.txt").read_bytes()).hexdigest(),
            "counts":dict(counts),"rows":rows}
    (args.directory/"manifest.json").write_text(json.dumps(data,indent=2)+"\n")
    print(f"Generated {len(rows)} programs; {len(counts)} categories")


if __name__=="__main__":
    main()
