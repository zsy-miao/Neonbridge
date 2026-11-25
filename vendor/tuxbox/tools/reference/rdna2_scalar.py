"""Independent, fail-closed RDNA2 ordinary scalar integer reference model.

Reference: AMD RDNA2 Shader ISA, sections 5 and 12 (SOP1/SOP2/SOPC).
This is a bounded subset, not a console emulator: no memory, vector lanes,
EXEC/VCC, control flow, floating point, hardware registers or timing.
Only s2..s9 are modelled. They must be initialized before reading.
"""
from dataclasses import dataclass
import operator

MASK = 0xffffffff
REGS = tuple(range(2, 10))
# Encoding numbers are ISA opcode fields, not TuxBox's decoded enums.
SOP2 = {
    0: "s_add_u32", 1: "s_sub_u32", 2: "s_add_i32", 3: "s_sub_i32",
    4: "s_addc_u32", 5: "s_subb_u32", 6: "s_min_i32", 7: "s_min_u32",
    8: "s_max_i32", 9: "s_max_u32", 10: "s_cselect_b32",
    14: "s_and_b32", 16: "s_or_b32", 18: "s_xor_b32",
    20: "s_andn2_b32", 22: "s_orn2_b32", 24: "s_nand_b32",
    26: "s_nor_b32", 28: "s_xnor_b32", 30: "s_lshl_b32",
    32: "s_lshr_b32", 34: "s_ashr_i32", 38: "s_mul_i32",
    11: "s_cselect_b64", 15: "s_and_b64", 17: "s_or_b64", 19: "s_xor_b64",
    21: "s_andn2_b64", 23: "s_orn2_b64", 25: "s_nand_b64",
    27: "s_nor_b64", 29: "s_xnor_b64", 31: "s_lshl_b64",
    33: "s_lshr_b64", 35: "s_ashr_i64",
}
SOP1 = {
    3: "s_mov_b32", 4: "s_mov_b64", 5: "s_cmov_b32", 6: "s_cmov_b64",
    7: "s_not_b32", 8: "s_not_b64", 11: "s_brev_b32", 12: "s_brev_b64",
    13: "s_bcnt0_i32_b32", 14: "s_bcnt0_i32_b64",
    15: "s_bcnt1_i32_b32", 16: "s_bcnt1_i32_b64",
    17: "s_ff0_i32_b32", 18: "s_ff0_i32_b64", 19: "s_ff1_i32_b32",
    21: "s_flbit_i32_b32", 22: "s_flbit_i32_b64", 23: "s_flbit_i32",
    24: "s_flbit_i32_i64", 25: "s_sext_i32_i8", 26: "s_sext_i32_i16",
    27: "s_bitset0_b32", 28: "s_bitset0_b64",
    29: "s_bitset1_b32", 30: "s_bitset1_b64", 52: "s_abs_i32",
}
SOPC = {i: f"s_cmp_{cmp}_{ty}" for i, (ty, cmp) in enumerate(
    (ty, cmp) for ty in ("i32", "u32") for cmp in ("eq", "lg", "gt", "ge", "lt", "le"))}
SOPC.update({12: "s_bitcmp0_b32", 13: "s_bitcmp1_b32",
             14: "s_bitcmp0_b64", 15: "s_bitcmp1_b64"})
EXTENDED = frozenset(SOP1[n] for n in (12,13,14,17,18,22,23,24,28,30)) | {
    SOPC[14], SOPC[15]}
FORMS = {name: (kind, code) for kind, table in (("sop1", SOP1), ("sop2", SOP2), ("sopc", SOPC))
         for code, name in table.items()}
END = 0xbf810000


def signed(x, bits=32):
    return x - (1 << bits) if x & (1 << (bits - 1)) else x


def widths(name):
    """Destination, first source, second source widths in words."""
    wide = name.endswith(("b64", "i64"))
    if name.startswith(("s_bcnt", "s_ff", "s_flbit")):
        return 1, 2 if wide else 1, 0
    if name.startswith("s_bitset"):
        return 2 if wide else 1, 1, 0
    if name.startswith("s_bitcmp"):
        return 0, 2 if wide else 1, 1
    if name.startswith("s_cmp"):
        return 0, 1, 1
    kind, _ = FORMS[name]
    return (2 if wide else 1, 2 if wide else 1,
            (1 if name.startswith(("s_lshl", "s_lshr", "s_ashr")) else 2 if wide else 1)
            if kind == "sop2" else 0)


def valid_reg(reg, width):
    return reg in REGS and reg + width - 1 in REGS and (width == 1 or reg % 2 == 0)


def encode(name, dest, a, b=0):
    kind, op = FORMS[name]
    if kind == "sop1":
        return 0xbe800000 | dest << 16 | op << 8 | a
    if kind == "sop2":
        return 0x80000000 | op << 23 | dest << 16 | b << 8 | a
    return 0xbf000000 | op << 16 | b << 8 | a


@dataclass(frozen=True)
class Instruction:
    name: str
    dst: int | None
    a: int
    b: int | None
    literal: int | None


def decode(words):
    if not 1 <= len(words) <= 512:
        raise ValueError("program must contain 1..512 words")
    if any(type(w) is not int or not 0 <= w <= MASK for w in words):
        raise ValueError("invalid native word")
    pc = 0
    while pc < len(words):
        word = words[pc]
        pc += 1
        if word == END:
            if pc != len(words):
                raise ValueError("trailing words after endpgm")
            return
        if word & 0xff800000 == 0xbe800000:
            name = SOP1.get((word >> 8) & 255)
            dst, a, b = (word >> 16) & 127, word & 255, None
        elif word & 0xff800000 == 0xbf000000:
            name = SOPC.get((word >> 16) & 127)
            dst, a, b = None, word & 255, (word >> 8) & 255
        elif word >> 30 == 2:
            name = SOP2.get((word >> 23) & 127)
            dst, a, b = (word >> 16) & 127, word & 255, (word >> 8) & 255
        else:
            raise ValueError(f"unsupported encoding at word {pc-1}")
        if name is None:
            raise ValueError(f"unsupported opcode at word {pc-1}")
        dw, aw, bw = widths(name)
        if dw and not valid_reg(dst, dw):
            raise ValueError("unsupported destination register")
        literal = None
        for selector, width in ((a, aw), (b, bw)):
            if not width:
                continue
            if valid_reg(selector, width):
                continue
            if 128 <= selector <= 208:
                continue
            if selector == 255 and width == 1:
                if literal is None:
                    if pc == len(words):
                        raise ValueError("missing literal")
                    literal = words[pc]
                    pc += 1
                continue
            raise ValueError("unsupported scalar source selector or pair")
        yield Instruction(name, dst, a, b, literal)
    raise ValueError("missing endpgm")


def execute(words, trace=None):
    """Return s2..s9 and SCC, using arbitrary precision integer arithmetic."""
    regs = {}
    scc = None
    def read(selector, width, literal):
        if 128 <= selector <= 208:
            value = selector - 128 if selector <= 192 else 192 - selector
            return value & ((1 << (32 * width)) - 1)
        if selector == 255:
            return literal
        try:
            return regs[selector] | (regs[selector+1] << 32 if width == 2 else 0)
        except KeyError as exc:
            raise ValueError("uninitialized scalar read") from exc

    def flag():
        if scc is None:
            raise ValueError("uninitialized SCC")
        return scc

    for ins in decode(words):
        n = ins.name
        dw, aw, bw = widths(n)
        a = read(ins.a, aw, ins.literal)
        b = read(ins.b, bw, ins.literal) if bw else 0
        bits = 32 * (dw or aw)
        mask = (1 << bits) - 1
        result = None
        op = n.split("_")[1]
        if op in ("add", "sub", "addc", "subb"):
            carry = flag() if op in ("addc", "subb") else 0
            if n.endswith("i32"):
                result = signed(a) + signed(b) if op == "add" else signed(a) - signed(b)
                scc = int(not -(1 << 31) <= result < 1 << 31)
            else:
                result = a + b + carry if op in ("add", "addc") else a - b - carry
                scc = int(not 0 <= result <= MASK)
        elif op in ("min", "max"):
            x,y = (signed(a), signed(b)) if n.endswith("i32") else (a,b)
            scc = int(x < y if op == "min" else x > y)
            result = a if scc else b
        elif op == "mul":
            result = a*b
        elif op == "mov":
            result = a
        elif op == "cmov":
            result = a if flag() else read(ins.dst, dw, None)
        elif op == "cselect":
            result = a if flag() else b
        elif op in ("and","or","xor","andn2","orn2","nand","nor","xnor","not"):
            result = {"and": a&b, "or": a|b, "xor": a^b, "andn2": a&~b,
                      "orn2": a|~b, "nand": ~(a&b), "nor": ~(a|b),
                      "xnor": ~(a^b), "not": ~a}[op] & mask
            scc = int(result != 0)
        elif op in ("lshl","lshr","ashr"):
            shift = b % bits
            result = (a << shift if op == "lshl" else a >> shift if op == "lshr"
                      else signed(a,bits) >> shift) & mask
            scc = int(result != 0)
        elif op == "brev":
            result = int(f"{a:0{bits}b}"[::-1], 2)
        elif op in ("bcnt0","bcnt1"):
            result = a.bit_count() if op == "bcnt1" else aw*32-a.bit_count()
            scc = int(result != 0)
        elif op in ("ff0","ff1"):
            result = next((i for i in range(aw*32) if (a >> i)&1 == int(op=="ff1")), MASK)
        elif op == "flbit":
            width = aw*32
            sign = a >> (width-1) if n in ("s_flbit_i32", "s_flbit_i32_i64") else 0
            result = next((i for i in range(width) if (a >> (width-1-i))&1 != sign), MASK)
        elif op == "sext":
            width = 8 if n.endswith("i8") else 16
            result = signed(a & ((1<<width)-1), width)
        elif op == "abs":
            result = abs(signed(a))
            scc = int(result != 0)
        elif op in ("bitset0","bitset1"):
            old = read(ins.dst, dw, None)
            bit = 1 << (a % bits)
            result = old | bit if op == "bitset1" else old & ~bit
        elif op in ("bitcmp0","bitcmp1"):
            scc = int((a >> (b % (aw*32)))&1 == int(op=="bitcmp1"))
        elif op == "cmp":
            relation, ty = n.split("_")[2:]
            x,y = (signed(a),signed(b)) if ty=="i32" else (a,b)
            scc = int({"eq":operator.eq,"lg":operator.ne,"gt":operator.gt,
                       "ge":operator.ge,"lt":operator.lt,"le":operator.le}[relation](x,y))
        else:
            raise ValueError(f"unimplemented {n}")
        if dw:
            if result is None:
                raise ValueError("missing result")
            regs[ins.dst] = result & MASK
            if dw == 2:
                regs[ins.dst+1] = (result >> 32) & MASK
        if trace is not None:
            trace.append({"instruction": ins.name, "dst": ins.dst, "src0": ins.a,
                          "src1": ins.b, "literal": ins.literal,
                          "registers": [regs.get(r) for r in REGS], "scc": scc})
    if set(regs) != set(REGS) or scc is None:
        raise ValueError("incomplete observable state")
    return [regs[r] for r in REGS] + [scc]

def main():
    import argparse
    import json
    from pathlib import Path
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("programs", type=Path, help="generated programs.txt")
    parser.add_argument("--case", required=True, help="exact six-digit case identifier")
    args = parser.parse_args()
    if len(args.case) != 6 or not args.case.isascii() or not args.case.isdigit():
        parser.error("invalid case ID")
    with args.programs.open(encoding="utf-8") as stream:
        matches = [line.split()[1:] for line in stream if line.split()[:1] == [args.case]]
    if len(matches) != 1:
        parser.error("case absent or duplicated")
    words = [int(word,16) for word in matches[0]]
    trace = []
    result = execute(words, trace)
    print(json.dumps({"case":args.case,"words":[f"{word:08x}" for word in words],
                      "register_order":[f"s{reg}" for reg in REGS]+["scc"],
                      "result":result,"trace":trace},indent=2))


if __name__ == "__main__":
    main()
