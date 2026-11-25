"""Traduction HLSL des instructions RDNA 2 à opérandes 64 bits (emu/rdna2_exec_wide.py).

Un double occupe deux registres 32 bits : asdouble(bas, haut) à la lecture, asuint(r, bas, haut) à
l'écriture. Seules les opérations que le HLSL accepte en double sont traduites (addition,
multiplication, fma, min, max, conversions) ; floor, sqrt, rcp… n'existent pas en double en HLSL
et restent nommées par Untranslatable, comme clamp/omod sur un résultat 64 bits.

Mesuré : sur WARP (Windows 11 26200), la conversion dynamique (double)asint(0) donne -0.0 (la
version u32 et la constante repliée par dxc donnent +0.0, comme RTX 4060) ; un test « x == 0 » est
replié par dxc, d'où la conversion par u32 avec le signe à part.
"""
import struct

from emu.rdna2_exec_wide import OPERATIONS
from emu.rdna2_hlsl_ops import NULL, Untranslatable, source, source_high

LITERAL, VGPR_FIRST = 255, 256
INLINE_DOUBLES = {240: 0.5, 241: -0.5, 242: 1.0, 243: -1.0, 244: 2.0, 245: -2.0, 246: 4.0, 247: -4.0,
                  248: 0.15915494309189532}    # 0x3FC45F306DC9C882 (ISA §6.2), comme rdna2_exec_cmp64

HELPERS = """bool IsNanF64(uint lo, uint hi) {
  uint h = hi & 0x7FFFFFFFu;
  return h > 0x7FF00000u || (h == 0x7FF00000u && lo != 0u);
}
uint SatF64ToI32(uint lo, uint hi) {
  double x = asdouble(lo, hi);
  if (IsNanF64(lo, hi)) return 0u;
  if (x >= 2147483647.0) return 0x7FFFFFFFu;
  if (x <= -2147483648.0) return 0x80000000u;
  return asuint((int)x);
}
uint SatF64ToU32(uint lo, uint hi) {
  double x = asdouble(lo, hi);
  if (IsNanF64(lo, hi) || x <= 0.0) return 0u;
  return x >= 4294967295.0 ? 0xFFFFFFFFu : (uint)x;
}
"""

# nom : expression HLSL ({a}, {b}, {c} : doubles ; {a_lo}/{a_hi}… : mots ; {u} : uint 32 bits)
DOUBLE = {
    "v_add_f64": "({a} + {b})", "v_mul_f64": "({a} * {b})", "v_fma_f64": "fma({a}, {b}, {c})",
    "v_min_f64": "min({a}, {b})", "v_max_f64": "max({a}, {b})",
    "v_cvt_f64_u32": "(double){u}",
    "v_cvt_f64_i32": "(asint({u}) < 0 ? -(double)(0u - {u}) : (double){u})",   # par u32 : voir plus haut
}
TO_32 = {
    "v_cvt_f32_f64": "asuint((float){a})",
    "v_cvt_i32_f64": "SatF64ToI32({a_lo}, {a_hi})",
    "v_cvt_u32_f64": "SatF64ToU32({a_lo}, {a_hi})",
}
SHIFTS = frozenset({"v_lshlrev_b64", "v_lshrrev_b64", "v_ashrrev_i64"})
TRANSLATED = frozenset(DOUBLE) | frozenset(TO_32) | SHIFTS | {"v_cvt_f64_f32"}


def _words(code):
    """(bas, haut) d'une source 64 bits."""
    if code == LITERAL:
        raise Untranslatable("littéral sur une source 64 bits")
    if code in INLINE_DOUBLES:
        bits = struct.unpack("<Q", struct.pack("<d", INLINE_DOUBLES[code]))[0]
        return f"0x{bits & 0xFFFF_FFFF:X}u", f"0x{bits >> 32:X}u"
    return ("0u", "0u") if code == NULL else (source(code), source_high(code))


def _float32(code, index, abs_bits, neg_bits):
    """Bits d'une source f32, abs/neg appliqués au bit de signe comme rdna2_exec_wide._modify."""
    bits = source(code)
    if abs_bits >> index & 1:
        bits = f"({bits} & 0x7FFFFFFFu)"
    if neg_bits >> index & 1:
        bits = f"({bits} ^ 0x80000000u)"
    return bits


def _double(code, index, abs_bits, neg_bits):
    low, high = _words(code)
    value = f"asdouble({low}, {high})"
    if abs_bits >> index & 1:
        value = f"abs({value})"
    if neg_bits >> index & 1:
        value = f"(-{value})"
    return value


def _f32_to_f64(bits, vdst):
    """v_cvt_f64_f32 en entiers, exacte (ISA : 0 ULP, dénormaux gardés) : signe gardé, NaN rendu
    silencieux avec sa charge utile comme dans l'interpréteur. Mesuré : (double)asfloat(x) rend
    0x7FFFFFFFE0000000 pour tout NaN sur RTX 4060 (signe et charge utile perdus)."""
    return (f"{{ uint cw = {bits}, cs = cw & 0x80000000u, ce = cw >> 23 & 0xFFu, cm = cw & 0x7FFFFFu, cx; "
            "if (ce == 0xFFu) cx = 0x7FF00000u | (cm != 0u ? 0x80000u : 0u); "
            "else if (ce != 0u) cx = (ce + 896u) << 20; "
            "else if (cm == 0u) cx = 0u; "
            "else { uint cn = 23u - firstbithigh(cm); cm = cm << cn & 0x7FFFFFu; cx = (897u - cn) << 20; } "
            f"v[{vdst}] = cm << 29; v[{vdst + 1}] = cs | cx | cm >> 3; }}")


def _shift(name, amount, low, high, vdst):
    if name == "v_lshlrev_b64":
        body = ("if (n >= 32u) { h = l << (n - 32u); l = 0u; } "
                "else if (n != 0u) { h = (h << n) | (l >> (32u - n)); l = l << n; }")
    elif name == "v_lshrrev_b64":
        body = ("if (n >= 32u) { l = h >> (n - 32u); h = 0u; } "
                "else if (n != 0u) { l = (l >> n) | (h << (32u - n)); h = h >> n; }")
    else:
        body = ("if (n >= 32u) { l = asuint(asint(h) >> (n - 32u)); h = asuint(asint(h) >> 31); } "
                "else if (n != 0u) { l = (l >> n) | (h << (32u - n)); h = asuint(asint(h) >> n); }")
    return f"{{ uint n = ({amount}) & 63u, l = {low}, h = {high}; {body} v[{vdst}] = l; v[{vdst + 1}] = h; }}"


def statements(name, codes, vdst, active, abs_bits=0, neg_bits=0, clamp=0, omod=0):
    """Instructions HLSL d'une instruction 64 bits (VOP1 ou VOP3)."""
    if name not in TRANSLATED:
        raise Untranslatable(f"non traduite en HLSL : {name} (double sans équivalent HLSL)")
    if clamp or omod:
        raise Untranslatable(f"non traduite en HLSL : {name} avec clamp/omod")
    kinds = OPERATIONS[name][0]
    if name in ("v_cvt_i32_f64", "v_cvt_u32_f64") and (abs_bits or neg_bits):
        raise Untranslatable(f"non traduite en HLSL : {name} avec abs/neg")
    if name in SHIFTS:
        low, high = _words(codes[1])
        return [f"if ({active}) {_shift(name, source(codes[0]), low, high, vdst)}"]
    values = {}
    for index, kind in enumerate(kinds):
        if kind == "f64":
            values["abc"[index]] = _double(codes[index], index, abs_bits, neg_bits)
            values[f"{'abc'[index]}_lo"], values[f"{'abc'[index]}_hi"] = _words(codes[index])
        elif kind == "f32":
            values["u"] = _float32(codes[index], index, abs_bits, neg_bits)
        else:
            values["u"] = source(codes[index])
    if name == "v_cvt_f64_f32":
        return [f"if ({active}) {_f32_to_f64(values['u'], vdst)}"]
    if name in TO_32:
        return [f"if ({active}) v[{vdst}] = {TO_32[name].format(**values)};"]
    expression = DOUBLE[name].format(**values)
    return [f"if ({active}) {{ uint lo, hi; asuint({expression}, lo, hi); v[{vdst}] = lo; v[{vdst + 1}] = hi; }}"]
