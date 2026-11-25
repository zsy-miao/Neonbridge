#!/usr/bin/env python3
"""LLVM-assembled common vector, scalar, float and comparison controls on a D3D12 GPU.

Each instruction is translated, run on dynamic edge and random inputs and
compared with an independent model of the console instruction. Instructions
the translator refuses are listed in the receipt; a wrong result fails the
check. Synthetic inputs and guard words; no game is used.
"""
import argparse
from fractions import Fraction
import json
import math
import operator
from pathlib import Path
import random
import struct
import subprocess
import sys
from tempfile import TemporaryDirectory

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tools"))
from check_packed_integer_gpu import assemble, digest, instrument, translate, INIT, STRIDE, GROUPS  # noqa: E402
from frosty_gpu_guest import run_gpu_guest  # noqa: E402

M = 0xffffffff
EDGE = [0, 1, 2, 31, 32, 33, 0xffff, 0x10000, 0x7fffff, 0x800000, 0xffffff, 0x1000000,
        0x7fffffff, 0x80000000, 0x80000001, 0xfffffffe, 0xffffffff, 0xaaaaaaaa]


def s32(value):
    return value - (1 << 32) if value >> 31 else value


def s24(value):
    value &= 0xffffff
    return value - (1 << 24) if value >> 23 else value


def leading_zeros(value):
    return 32 - value.bit_length()


def bit_field(a, b, c):
    return (a >> (b & 31)) & ((1 << (c & 31)) - 1)


def signed_bit_field(a, b, c):
    offset, width = b & 31, c & 31
    if not width:
        return 0
    if offset + width < 32:
        field = bit_field(a, b, c)
        return (field - (1 << width) if field >> (width - 1) else field) & M
    return (s32(a) >> offset) & M


def float_bits(value):
    return struct.unpack("<I", struct.pack("<f", float(value)))[0]


def median(values):
    return sorted(values)[1]


# name: (source count, model of the 32-bit result)
OPERATIONS = {
    "v_not_b32": (1, lambda a, b, c: ~a & M),
    "v_bfrev_b32": (1, lambda a, b, c: int(f"{a:032b}"[::-1], 2)),
    "v_ffbh_u32": (1, lambda a, b, c: leading_zeros(a) if a else M),
    "v_ffbl_b32": (1, lambda a, b, c: (a & -a).bit_length() - 1 if a else M),
    "v_ffbh_i32": (1, lambda a, b, c: M if a in (0, M) else leading_zeros(a ^ M if a >> 31 else a)),
    "v_cvt_f32_u32": (1, lambda a, b, c: float_bits(a)),
    "v_cvt_f32_i32": (1, lambda a, b, c: float_bits(s32(a))),
    "v_add_nc_u32": (2, lambda a, b, c: (a + b) & M),
    "v_sub_nc_u32": (2, lambda a, b, c: (a - b) & M),
    "v_subrev_nc_u32": (2, lambda a, b, c: (b - a) & M),
    "v_and_b32": (2, lambda a, b, c: a & b),
    "v_or_b32": (2, lambda a, b, c: a | b),
    "v_xor_b32": (2, lambda a, b, c: a ^ b),
    "v_xnor_b32": (2, lambda a, b, c: ~(a ^ b) & M),
    "v_lshlrev_b32": (2, lambda a, b, c: (b << (a & 31)) & M),
    "v_lshrrev_b32": (2, lambda a, b, c: b >> (a & 31)),
    "v_ashrrev_i32": (2, lambda a, b, c: (s32(b) >> (a & 31)) & M),
    "v_min_u32": (2, lambda a, b, c: min(a, b)),
    "v_max_u32": (2, lambda a, b, c: max(a, b)),
    "v_min_i32": (2, lambda a, b, c: min(s32(a), s32(b)) & M),
    "v_max_i32": (2, lambda a, b, c: max(s32(a), s32(b)) & M),
    "v_mul_u32_u24": (2, lambda a, b, c: ((a & 0xffffff) * (b & 0xffffff)) & M),
    "v_mul_i32_i24": (2, lambda a, b, c: (s24(a) * s24(b)) & M),
    "v_mul_hi_u32_u24": (2, lambda a, b, c: (((a & 0xffffff) * (b & 0xffffff)) >> 32) & M),
    "v_mul_hi_i32_i24": (2, lambda a, b, c: ((s24(a) * s24(b)) >> 32) & M),
    "v_mul_lo_u32": (2, lambda a, b, c: (a * b) & M),
    "v_mul_hi_u32": (2, lambda a, b, c: (a * b) >> 32),
    "v_mul_hi_i32": (2, lambda a, b, c: ((s32(a) * s32(b)) >> 32) & M),
    "v_bfe_u32": (3, bit_field),
    "v_bfe_i32": (3, signed_bit_field),
    "v_bfi_b32": (3, lambda a, b, c: ((a & b) | (~a & c)) & M),
    "v_alignbit_b32": (3, lambda a, b, c: (((a << 32) | b) >> (c & 31)) & M),
    "v_alignbyte_b32": (3, lambda a, b, c: (((a << 32) | b) >> (8 * (c & 3))) & M),
    "v_add3_u32": (3, lambda a, b, c: (a + b + c) & M),
    "v_lshl_add_u32": (3, lambda a, b, c: ((a << (b & 31)) + c) & M),
    "v_add_lshl_u32": (3, lambda a, b, c: ((a + b) << (c & 31)) & M),
    "v_lshl_or_b32": (3, lambda a, b, c: ((a << (b & 31)) | c) & M),
    "v_and_or_b32": (3, lambda a, b, c: (a & b) | c),
    "v_or3_b32": (3, lambda a, b, c: a | b | c),
    "v_xor3_b32": (3, lambda a, b, c: a ^ b ^ c),
    "v_min3_u32": (3, lambda a, b, c: min(a, b, c)),
    "v_max3_u32": (3, lambda a, b, c: max(a, b, c)),
    "v_med3_u32": (3, lambda a, b, c: median([a, b, c])),
    "v_min3_i32": (3, lambda a, b, c: min(s32(a), s32(b), s32(c)) & M),
    "v_max3_i32": (3, lambda a, b, c: max(s32(a), s32(b), s32(c)) & M),
    "v_med3_i32": (3, lambda a, b, c: median([s32(a), s32(b), s32(c)]) & M),
    "v_mad_u32_u24": (3, lambda a, b, c: ((a & 0xffffff) * (b & 0xffffff) + c) & M),
    "v_mad_i32_i24": (3, lambda a, b, c: (s24(a) * s24(b) + s32(c)) & M),
    "v_sad_u32": (3, lambda a, b, c: (abs(a - b) + c) & M),
}


def signed_overflow(value):
    return not -2**31 <= value < 2**31


def scalar_field(a, b, signed):
    """`s_bfe`: the offset is in bits 4:0 of the second source, the width in 22:16."""
    offset, width = b & 31, min((b >> 16) & 127, 32)
    if not width:
        return 0
    if not signed:
        return (a >> offset) & ((1 << width) - 1)
    if offset + width < 32:
        field = (a >> offset) & ((1 << width) - 1)
        return (field - (1 << width) if field >> (width - 1) else field) & M
    return (s32(a) >> offset) & M


def nonzero(value):
    return value, int(value != 0)


# name: (source count, model returning the result and SCC, or None when the
# instruction leaves SCC alone)
SCALAR = {
    "s_add_u32": (2, lambda a, b: ((a + b) & M, int(a + b > M))),
    "s_sub_u32": (2, lambda a, b: ((a - b) & M, int(b > a))),
    "s_add_i32": (2, lambda a, b: ((a + b) & M, int(signed_overflow(s32(a) + s32(b))))),
    "s_sub_i32": (2, lambda a, b: ((a - b) & M, int(signed_overflow(s32(a) - s32(b))))),
    "s_min_u32": (2, lambda a, b: (min(a, b), int(a < b))),
    "s_max_u32": (2, lambda a, b: (max(a, b), int(a > b))),
    "s_min_i32": (2, lambda a, b: (min(s32(a), s32(b)) & M, int(s32(a) < s32(b)))),
    "s_max_i32": (2, lambda a, b: (max(s32(a), s32(b)) & M, int(s32(a) > s32(b)))),
    "s_and_b32": (2, lambda a, b: nonzero(a & b)),
    "s_or_b32": (2, lambda a, b: nonzero(a | b)),
    "s_xor_b32": (2, lambda a, b: nonzero(a ^ b)),
    "s_andn2_b32": (2, lambda a, b: nonzero(a & ~b & M)),
    "s_orn2_b32": (2, lambda a, b: nonzero((a | ~b) & M)),
    "s_nand_b32": (2, lambda a, b: nonzero(~(a & b) & M)),
    "s_nor_b32": (2, lambda a, b: nonzero(~(a | b) & M)),
    "s_xnor_b32": (2, lambda a, b: nonzero(~(a ^ b) & M)),
    "s_lshl_b32": (2, lambda a, b: nonzero((a << (b & 31)) & M)),
    "s_lshr_b32": (2, lambda a, b: nonzero(a >> (b & 31))),
    "s_ashr_i32": (2, lambda a, b: nonzero((s32(a) >> (b & 31)) & M)),
    "s_bfm_b32": (2, lambda a, b: ((((1 << (a & 31)) - 1) << (b & 31)) & M, None)),
    "s_mul_i32": (2, lambda a, b: ((a * b) & M, None)),
    "s_mul_hi_u32": (2, lambda a, b: ((a * b) >> 32, None)),
    "s_mul_hi_i32": (2, lambda a, b: (((s32(a) * s32(b)) >> 32) & M, None)),
    "s_bfe_u32": (2, lambda a, b: nonzero(scalar_field(a, b, False))),
    "s_bfe_i32": (2, lambda a, b: nonzero(scalar_field(a, b, True))),
    "s_lshl1_add_u32": (2, lambda a, b: (((a << 1) + b) & M, int((a << 1) + b > M))),
    "s_lshl2_add_u32": (2, lambda a, b: (((a << 2) + b) & M, int((a << 2) + b > M))),
    "s_lshl3_add_u32": (2, lambda a, b: (((a << 3) + b) & M, int((a << 3) + b > M))),
    "s_lshl4_add_u32": (2, lambda a, b: (((a << 4) + b) & M, int((a << 4) + b > M))),
    "s_pack_ll_b32_b16": (2, lambda a, b: ((a & 0xffff) | (b & 0xffff) << 16, None)),
    "s_pack_lh_b32_b16": (2, lambda a, b: ((a & 0xffff) | (b & 0xffff0000), None)),
    "s_pack_hh_b32_b16": (2, lambda a, b: ((a >> 16) | (b & 0xffff0000), None)),
    "s_not_b32": (1, lambda a: nonzero(~a & M)),
    "s_brev_b32": (1, lambda a: (int(f"{a:032b}"[::-1], 2), None)),
    "s_bcnt0_i32_b32": (1, lambda a: nonzero(32 - bin(a).count("1"))),
    "s_bcnt1_i32_b32": (1, lambda a: nonzero(bin(a).count("1"))),
    "s_ff1_i32_b32": (1, lambda a: ((a & -a).bit_length() - 1 if a else M, None)),
    "s_flbit_i32_b32": (1, lambda a: (leading_zeros(a) if a else M, None)),
    "s_sext_i32_i8": (1, lambda a: (((a & 0xff) - (0x100 if a & 0x80 else 0)) & M, None)),
    "s_sext_i32_i16": (1, lambda a: (((a & 0xffff) - (0x10000 if a & 0x8000 else 0)) & M, None)),
}
OUTPUTS = 8

INFINITY = float("inf")


def to_float(bits):
    return struct.unpack("<f", struct.pack("<I", bits))[0]


def to_bits(value):
    """The nearest f32 of a Python float, rounding to even and overflowing to infinity."""
    try:
        return struct.unpack("<I", struct.pack("<f", value))[0]
    except OverflowError:
        return 0xff800000 if value < 0 else 0x7f800000


def is_nan(bits):
    return bits & 0x7fffffff > 0x7f800000


def is_denormal(bits):
    return 0 < bits & 0x7fffffff < 0x00800000


class Float:
    """An expected f32. Any NaN matches a NaN. A denormal may be flushed to a
    zero of the same sign, as the float mode decides. `ulps` allows for
    approximate console instructions, and `any_zero` for an undefined zero sign."""

    def __init__(self, bits, ulps=0, any_zero=False):
        self.bits, self.ulps, self.any_zero = bits, ulps, any_zero

    def matches(self, got):
        want = self.bits
        if is_nan(want):
            return is_nan(got)
        if got == want or (is_denormal(want) and got == want & 0x80000000):
            return True
        if self.any_zero and not want & 0x7fffffff and not got & 0x7fffffff:
            return True
        same_sign = not (got ^ want) & 0x80000000
        return same_sign and not is_nan(got) and abs((got & 0x7fffffff) - (want & 0x7fffffff)) <= self.ulps

    def __repr__(self):
        return f"{self.bits:#010x}" + (f"~{self.ulps}ulp" if self.ulps else "")


def matches(got, want):
    return want.matches(got) if isinstance(want, Float) else got == want


def fused(a, b, c):
    """a * b + c rounded once."""
    values = [to_float(x) for x in (a, b, c)]
    if not all(math.isfinite(v) for v in values):
        return to_bits(values[0] * values[1] + values[2])
    exact = Fraction(values[0]) * Fraction(values[1]) + Fraction(values[2])
    if not exact:
        return to_bits(values[0] * values[1] + values[2])
    near = to_bits(float(exact))
    candidates = [near + step for step in (-1, 0, 1)
                  if 0 <= (near & 0x7fffffff) + step <= 0x7f7fffff]

    def distance(bits):
        return abs(Fraction(to_float(bits)) - exact), bits & 1

    if near & 0x7fffffff >= 0x7f800000:
        return near
    return min(candidates, key=distance)


def arithmetic(function, ulps=0):
    def model(a, b, c):
        try:
            return Float(to_bits(function(to_float(a), to_float(b), to_float(c))), ulps)
        except (ValueError, ZeroDivisionError):
            return Float(0x7fc00000)
    return model


def extreme(pick, count):
    """min or max of `count` sources, ignoring NaN as the console's IEEE form does."""
    def model(a, b, c):
        numbers = [to_float(x) for x in (a, b, c)[:count] if not is_nan(x)]
        return Float(to_bits(pick(numbers)) if numbers else 0x7fc00000, any_zero=True)
    return model


def whole(function):
    def model(a, b, c):
        value = to_float(a)
        if not math.isfinite(value):
            return Float(a)
        result = function(value)
        # A result of zero keeps the input's sign.
        return Float(to_bits(math.copysign(0.0, value) if result == 0 else float(result)))
    return model


def to_integer(function, low, high):
    def model(a, b, c):
        value = to_float(a)
        if math.isnan(value):
            return 0
        if math.isinf(value):
            return (high if value > 0 else low) & M
        return max(low, min(high, function(value))) & M
    return model


# name: (source count, model returning a word or an expected `Float`)
FLOATS = {
    "v_add_f32": (2, arithmetic(lambda a, b, c: a + b)),
    "v_sub_f32": (2, arithmetic(lambda a, b, c: a - b)),
    "v_subrev_f32": (2, arithmetic(lambda a, b, c: b - a)),
    "v_mul_f32": (2, arithmetic(lambda a, b, c: a * b)),
    "v_min_f32": (2, extreme(min, 2)),
    "v_max_f32": (2, extreme(max, 2)),
    "v_min3_f32": (3, extreme(min, 3)),
    "v_max3_f32": (3, extreme(max, 3)),
    "v_fma_f32": (3, lambda a, b, c: Float(fused(a, b, c))),
    "v_floor_f32": (1, whole(math.floor)),
    "v_ceil_f32": (1, whole(math.ceil)),
    "v_trunc_f32": (1, whole(math.trunc)),
    "v_rndne_f32": (1, whole(round)),
    "v_sqrt_f32": (1, arithmetic(lambda a, b, c: math.sqrt(a) if a >= 0 else float("nan"), ulps=1)),
    "v_rcp_f32": (1, arithmetic(lambda a, b, c: 1 / a if a else math.copysign(INFINITY, a), ulps=1)),
    "v_cvt_i32_f32": (1, to_integer(math.trunc, -2**31, 2**31 - 1)),
    "v_cvt_u32_f32": (1, to_integer(math.trunc, 0, M)),
    "v_cvt_flr_i32_f32": (1, to_integer(math.floor, -2**31, 2**31 - 1)),
    "v_cvt_rpi_i32_f32": (1, to_integer(lambda v: math.floor(v + 0.5), -2**31, 2**31 - 1)),
}
FLOAT_EDGE = [0x00000000, 0x80000000, 0x3f800000, 0xbf800000, 0x3f000000, 0xbf000000, 0x3fc00000,
              0xc0200000, 0x4b000000, 0x4effffff, 0x4f000000, 0xcf000000, 0x4f800000, 0x7f7fffff,
              0xff7fffff, 0x7f800000, 0xff800000, 0x7fc00000, 0x00800000, 0x3effffff]

RELATIONS = {"eq": operator.eq, "ne": operator.ne, "lt": operator.lt, "gt": operator.gt,
             "le": operator.le, "ge": operator.ge, "lg": operator.ne}


def float_relation(name):
    def model(a, b, c):
        x, y = to_float(a), to_float(b)
        unordered = math.isnan(x) or math.isnan(y)
        if name in ("o", "u"):
            return int(unordered == (name == "u"))
        if name == "lg":
            return int(not unordered and x != y)
        if name == "neq":
            return int(unordered or x != y)
        if name.startswith("n"):
            return int(not RELATIONS[name[1:]](x, y) or unordered)
        return int(not unordered and RELATIONS[name](x, y))
    return model


# Vector comparisons: the lane's bit of VCC, selected into 0 or 1.
COMPARES = {
    **{f"v_cmp_{name}_u32": (2, lambda a, b, c, name=name: int(RELATIONS[name](a, b)))
       for name in ("eq", "ne", "lt", "gt", "le", "ge")},
    **{f"v_cmp_{name}_i32": (2, lambda a, b, c, name=name: int(RELATIONS[name](s32(a), s32(b))))
       for name in ("eq", "ne", "lt", "gt", "le", "ge")},
}
FLOAT_COMPARES = {f"v_cmp_{name}_f32": (2, float_relation(name))
                  for name in ("eq", "lt", "gt", "le", "ge", "lg", "o", "u", "neq", "nlt", "ngt", "nle", "nge")}
SCALAR_COMPARES = {
    **{f"s_cmp_{name}_u32": (2, lambda a, b, name=name: int(RELATIONS[name](a, b)))
       for name in ("eq", "lg", "lt", "gt", "le", "ge")},
    **{f"s_cmp_{name}_i32": (2, lambda a, b, name=name: int(RELATIONS[name](s32(a), s32(b))))
       for name in ("eq", "lg", "lt", "gt", "le", "ge")},
}


def is_float(name):
    return name in FLOATS or name in FLOAT_COMPARES


def count(name):
    """Result words one instruction produces: scalar ones also report SCC."""
    if name not in SCALAR:
        return 1
    return 1 if SCALAR[name][1](*[1] * SCALAR[name][0])[1] is None else 2


def lines_for(name, destination):
    """Assembly leaving the instruction's results from `v{destination}` on."""
    if name in OPERATIONS or name in FLOATS:
        sources = ", ".join(f"v{4 + j}" for j in range({**OPERATIONS, **FLOATS}[name][0]))
        return [f"{name} v{destination}, {sources}"]
    if name in COMPARES or name in FLOAT_COMPARES:
        return [f"{name} vcc_lo, v4, v5", f"v_cndmask_b32_e64 v{destination}, 0, 1, vcc_lo"]
    if name in SCALAR_COMPARES:
        return ["v_readfirstlane_b32 s20, v4", "v_readfirstlane_b32 s21, v5", f"{name} s20, s21",
                "s_cselect_b32 s24, 1, 0", f"v_mov_b32_e32 v{destination}, s24"]
    arity = SCALAR[name][0]
    sources = ", ".join(f"s{20 + j}" for j in range(arity))
    # Each input is uniform: a dispatch group holds one lane.
    program = [f"v_readfirstlane_b32 s{20 + j}, v{4 + j}" for j in range(arity)]
    # Give SCC a known value, so an instruction that should keep it is caught.
    program += ["s_cmp_eq_u32 s20, s20", f"{name} s24, {sources}"]
    if count(name) == 2:
        program += ["s_cselect_b32 s25, 1, 0", f"v_mov_b32_e32 v{destination + 1}, s25"]
    return program + [f"v_mov_b32_e32 v{destination}, s24"]


def results(name, values):
    for table in (OPERATIONS, FLOATS, COMPARES, FLOAT_COMPARES):
        if name in table:
            return [table[name][1](*values)]
    if name in SCALAR_COMPARES:
        return [SCALAR_COMPARES[name][1](*values[:2])]
    arity, model = SCALAR[name]
    value, flag = model(*values[:arity])
    return [value] if flag is None else [value, flag]


def program(chunk):
    out, destination = inputs(), 7
    for name in chunk:
        out += lines_for(name, destination)
        destination += count(name)
    return out + ["s_endpgm"]


def chunks_of(names):
    chunk, used = [], 0
    # Float and integer instructions need different inputs, so a shader holds one kind.
    for name in sorted(names, key=is_float):
        if used + count(name) > OUTPUTS or (chunk and is_float(name) != is_float(chunk[0])):
            yield chunk
            chunk, used = [], 0
        chunk.append(name)
        used += count(name)
    if chunk:
        yield chunk


def probe(names, emitter, llvm_mc):
    """Split `names` into those translated and those refused or unassembled."""
    supported, refused = [], {}
    for name in names:
        try:
            words = assemble(program([name]), llvm_mc)
        except (ValueError, subprocess.CalledProcessError) as error:
            refused[name] = f"not assembled: {str(error)[:120]}"
            continue
        got = translate(emitter, words)
        if got.returncode == 65:
            refused[name] = got.stderr.strip()
        elif got.returncode:
            raise RuntimeError(f"emitter failed on {name}: {got.returncode} {got.stderr[-300:]}")
        else:
            supported.append(name)
    return supported, refused


def inputs():
    return [f"v_mov_b32_e32 v{4 + j}, 0x{value:x}" for j, value in enumerate(INIT)]


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    for n in ("emitter", "llvm-mc", "dxc", "dxv", "provider", "proton", "prefix", "compiler-dir", "work", "output"):
        ap.add_argument("--" + n, type=Path, required=True)
    ap.add_argument("--adapter", required=True)
    ap.add_argument("--rounds", type=int, default=8, help=f"dispatches per shader, {GROUPS} inputs each")
    a = ap.parse_args()
    if a.output.exists():
        ap.error("choose a fresh receipt")
    a.work.mkdir(parents=True, exist_ok=False)
    rng = random.Random(0x414c5531)
    every = [*OPERATIONS, *SCALAR, *COMPARES, *SCALAR_COMPARES, *FLOATS, *FLOAT_COMPARES]
    supported, refused = probe(every, a.emitter, a.llvm_mc)
    expected, chunks = {}, {}
    with TemporaryDirectory(dir=a.work) as td:
        trial = Path(td)
        guest = ROOT / "tools/reference/independent-shaders/compute.cpp"
        (trial / "guest.cpp").write_text(guest.read_text())
        (trial / "context.h").write_text((guest.parent / "context.h").read_text().replace(
            '"../compute_buffers.h"', '"' + str(ROOT / "tools/reference/compute_buffers.h") + '"'))
        for number, chunk in enumerate(chunks_of(supported)):
            got = translate(a.emitter, assemble(program(chunk), a.llvm_mc))
            got.check_returncode()
            outputs = sum(count(name) for name in chunk)
            shader = trial / f"alu{number:02}.hlsl"
            shader.write_text(instrument(got.stdout, outputs))
            blob = shader.with_suffix(".cso")
            subprocess.run([a.dxc, "-T", "cs_6_0", "-E", "main", "-O3", "-Fo", blob, shader],
                           check=True, capture_output=True, timeout=30)
            subprocess.run([a.dxv, blob], check=True, capture_output=True, timeout=15)
            for turn in range(a.rounds):
                label = f"alu{number:02}r{turn:02}"
                (trial / (label + ".cso")).write_bytes(blob.read_bytes())
                seed = [rng.getrandbits(32) for _ in range(384)]
                want = seed.copy()
                for lane in range(GROUPS):
                    index = turn * GROUPS + lane
                    edges = FLOAT_EDGE if is_float(chunk[0]) else EDGE
                    # Edge values first, rotated so each source sees each of them.
                    if index < len(edges):
                        values = [edges[(index + 7 * j) % len(edges)] for j in range(3)]
                    elif is_float(chunk[0]):
                        # Ordinary magnitudes of both signs, so results stay finite
                        # and rounding matters. Some pairs are equal.
                        values = [to_bits(rng.uniform(-1, 1) * 2.0 ** rng.randint(-30, 30)) for _ in range(3)]
                        if rng.random() < 0.2:
                            values[1] = values[0]
                    else:
                        values = [rng.getrandbits(32) for _ in range(3)]
                        if rng.random() < 0.1:
                            values[1] = values[0]
                    offset = 96 + STRIDE * lane
                    seed[offset:offset + 3] = values
                    want[offset:offset + 3] = values
                    row = [word for name in chunk for word in results(name, values)]
                    want[offset + 3:offset + 3 + outputs] = row
                (trial / (label + ".seed")).write_bytes(struct.pack("<384I", *seed))
                expected[label] = want
                chunks[label] = chunk
        (trial / "cases.txt").write_text("".join(f"{n} 4 structured {GROUPS}\n" for n in expected))
        _, status, fields = run_gpu_guest(trial / "guest.cpp", "basic-alu.exe", trial, a.work, a.proton,
                                          a.prefix, a.provider, a.compiler_dir)
        if status or fields.get("adapter") != a.adapter:
            raise RuntimeError(f"guest failed: {status} {fields}")
        wrong = {}
        for label, chunk in chunks.items():
            observed = struct.unpack("<384I", (trial / (label + ".bin")).read_bytes())
            wanted = expected[label]
            for index, (got_word, want_word) in enumerate(zip(observed, wanted)):
                if matches(got_word, want_word):
                    continue
                lane, column = divmod(index - 96, STRIDE)
                owners = [name for name in chunk for _ in range(count(name))]
                name = owners[column - 3] if 0 <= lane < GROUPS and 3 <= column < 3 + len(owners) else "guard"
                wrong.setdefault(name, {"shader": label, "inputs": list(wanted[96 + STRIDE * lane:99 + STRIDE * lane]),
                                        "got": got_word, "wanted": repr(want_word)})
    receipt = {"schema": 1, "adapter": a.adapter, "tools_sha256": {n: digest(getattr(a, n))
                                                                   for n in ("emitter", "dxc", "provider")},
               "checked": supported, "refused": refused, "wrong": wrong, "inputs": a.rounds * GROUPS}
    a.output.write_text(json.dumps(receipt, indent=2) + "\n")
    for name, reason in refused.items():
        print(f"refused {name}: {reason[:150]}")
    for name, detail in wrong.items():
        print(f"WRONG {name}: {detail}")
    print(f"{'FAIL' if wrong else 'PASS'} {len(supported)} instructions on {a.rounds * GROUPS} inputs, "
          f"{len(wrong)} wrong, {len(refused)} refused")
    return int(bool(wrong))


if __name__ == "__main__":
    raise SystemExit(main())
