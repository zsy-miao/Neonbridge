"""Traduction HLSL de la division IEEE RDNA 2 (emu/rdna2_exec_div.py) : v_div_scale_f32,
v_div_fmas_f32, v_div_fixup_f32 et v_div_fixup_f64.

Le GPU du PC peut vider les dénormaux f32 (règle D3D) : les tests et les mises à l'échelle se font
sur les bits, ou en double, où les valeurs f32 et leurs produits restent normaux. v_div_fmas_f32 :
produit exact en double, somme arrondie « à l'impair » (erreur de TwoSum) puis arrondie au plus près
en f32 par F64ToF32 : un seul arrondi effectif, dénormaux compris, comme l'interpréteur.
v_div_fmas_f64 reste nommée par Untranslatable : quand VCC divise par 2^128, le résultat est
dénormal et le fma HLSL l'arrondirait deux fois. v_div_scale_f64 aussi (UNTRANSLATED_REASONS).
"""
from emu.rdna2_exec_div import OPERATIONS
from emu.rdna2_hlsl_ops import Untranslatable, scalar_target, source
from emu.rdna2_hlsl_wide import _words

TRANSLATED = frozenset({"v_div_scale_f32", "v_div_fmas_f32", "v_div_fixup_f32", "v_div_fixup_f64"})
UNTRANSLATED_REASONS = {
    "v_div_fmas_f64": "arrondi d'un résultat dénormal f64",
    # L'interpréteur refuse la branche « 1/S1 dénormal » (|S1| > 2^1022) : pas de référence pour le GPU,
    # qui ne peut pas refuser une valeur à l'exécution ; et « S2/S1 dénormal » demande une comparaison
    # exacte de produits de 106 bits, sans type plus large que le double en HLSL.
    "v_div_scale_f64": "branche « 1/S1 dénormal » sans référence, test du quotient dénormal au-delà du double",
}
SIGN = "0x80000000u"

HELPERS = """double F32ToF64(uint a) {
  // Conversion exacte sur les bits (le GPU peut vider un dénormal f32 converti par (double)).
  uint s = a & 0x80000000u, e = (a >> 23) & 0xFFu, m = a & 0x7FFFFFu;
  if (e == 0u) return m == 0u ? asdouble(0u, s) : (s != 0u ? -1.0 : 1.0) * (double)m * asdouble(0u, 0x36A00000u);
  return asdouble(m << 29, s | ((e + 896u) << 20) | (m >> 3));
}
uint F64ToF32(double x) {
  // Arrondi au plus près pair d'un double vers f32, dénormaux et dépassement compris.
  uint lo, hi; asuint(x, lo, hi);
  uint s = hi & 0x80000000u, ef = (hi >> 20) & 0x7FFu;
  if (ef == 0u) return s;
  int e = int(ef) - 1023;
  uint sig = 0x80000000u | ((hi & 0xFFFFFu) << 11) | (lo >> 21);
  bool sticky = (lo & 0x1FFFFFu) != 0u;
  uint shift = e < -126 ? uint(8 + (-126 - e)) : 8u;
  if (shift > 32u) return s;
  uint kept = shift == 32u ? 0u : sig >> shift;
  uint rest = shift == 32u ? sig : sig & ((1u << shift) - 1u);
  uint half = 1u << (shift - 1u);
  if (rest > half || (rest == half && (sticky || (kept & 1u) != 0u))) kept += 1u;
  if (e < -126) return s | kept;
  if (kept == 0x1000000u) { kept = 0x800000u; e += 1; }
  if (e > 127) return s | 0x7F800000u;
  return s | (uint(e + 127) << 23) | (kept & 0x7FFFFFu);
}
bool EqF32(uint a, uint b) {
  return !IsNanF32(a) && !IsNanF32(b) && (a == b || ((a | b) & 0x7FFFFFFFu) == 0u);
}
uint ScaleF32(uint a) {
  // ldexp(a, 64) ; infini et NaN inchangés.
  return ((a >> 23) & 0xFFu) == 0xFFu ? a : F64ToF32(F32ToF64(a) * asdouble(0u, 0x43F00000u));
}
bool QuotientIsDenormalF32(uint n, uint d) {
  // |n / d| arrondi est un dénormal non nul : 2^-150 < |n / d| < (2^24 - 1) × 2^-150 (produits exacts).
  precise double x = F32ToF64(n & 0x7FFFFFFFu), y = F32ToF64(d & 0x7FFFFFFFu);
  precise double low = y * asdouble(0u, 0x36900000u), high = y * ((double)0xFFFFFFu * asdouble(0u, 0x36900000u));
  return x > low && x < high;
}
uint DivScaleF32(uint s0, uint s1, uint s2, out bool vcc) {
  vcc = false;
  uint a1 = s1 & 0x7FFFFFFFu, a2 = s2 & 0x7FFFFFFFu, e1 = a1 >> 23, e2 = a2 >> 23;
  if (a1 == 0u || a2 == 0u) return 0x7FC00000u;
  if (int(e2) - int(e1) >= 96) { vcc = true; return EqF32(s0, s1) ? ScaleF32(s0) : s0; }
  if (e1 == 0u) return ScaleF32(s0);
  if (e1 != 0xFFu && e2 != 0xFFu && QuotientIsDenormalF32(s2, s1)) { vcc = true; return EqF32(s0, s2) ? ScaleF32(s0) : s0; }
  return e2 <= 23u ? ScaleF32(s0) : s0;
}
uint DivFmasF32(uint a, uint b, uint c, bool vcc) {
  bool ia = (a & 0x7FFFFFFFu) == 0x7F800000u, ib = (b & 0x7FFFFFFFu) == 0x7F800000u;
  bool ic = (c & 0x7FFFFFFFu) == 0x7F800000u;
  if (IsNanF32(a) || IsNanF32(b) || IsNanF32(c) || ia || ib || ic) {
    uint ps = (a ^ b) & 0x80000000u;
    bool za = (a & 0x7FFFFFFFu) == 0u, zb = (b & 0x7FFFFFFFu) == 0u;
    if (IsNanF32(a) || IsNanF32(b) || IsNanF32(c) || (ia && zb) || (ib && za)
        || ((ia || ib) && ic && ps != (c & 0x80000000u))) return 0x7FC00000u;
    return ia || ib ? ps | 0x7F800000u : c;
  }
  precise double p = F32ToF64(a) * F32ToF64(b), z = F32ToF64(c);
  precise double r = p + z;
  precise double t = r - p;
  precise double err = (p - (r - t)) + (z - t);
  uint lo, hi; asuint(r, lo, hi);
  if (err != 0.0 && (lo & 1u) == 0u) {              // arrondi à l'impair : le bit 0 garde l'inexactitude
    if ((err > 0.0) == (r > 0.0)) lo |= 1u; else { if (lo == 0u) hi -= 1u; lo -= 1u; }
    r = asdouble(lo, hi);
  }
  if (vcc) r *= (c & 0x7FFFFFFFu) >= 0x40000000u ? asdouble(0u, 0x43F00000u) : asdouble(0u, 0x3BF00000u);
  return F64ToF32(r);
}
uint DivFixupF32(uint q, uint d, uint n) {
  uint sign = (d ^ n) & 0x80000000u, a1 = d & 0x7FFFFFFFu, a2 = n & 0x7FFFFFFFu;
  if (a2 > 0x7F800000u) return n | 0x00400000u;
  if (a1 > 0x7F800000u) return d | 0x00400000u;
  if ((a1 == 0u && a2 == 0u) || (a1 == 0x7F800000u && a2 == 0x7F800000u)) return 0xFFC00000u;
  if (a1 == 0u || a2 == 0x7F800000u) return sign | 0x7F800000u;
  if (a1 == 0x7F800000u || a2 == 0u) return sign;
  if (int(a2 >> 23) - int(a1 >> 23) < -150) return sign;
  return IsNanF32(q) ? sign | 0x7F800000u : sign | (q & 0x7FFFFFFFu);
}
uint2 DivFixupF64(uint2 q, uint2 d, uint2 n) {
  uint sign = (d.y ^ n.y) & 0x80000000u, dh = d.y & 0x7FFFFFFFu, nh = n.y & 0x7FFFFFFFu;
  bool dzero = dh == 0u && d.x == 0u, nzero = nh == 0u && n.x == 0u;
  bool dinf = dh == 0x7FF00000u && d.x == 0u, ninf = nh == 0x7FF00000u && n.x == 0u;
  if (IsNanF64(n.x, n.y)) return uint2(n.x, n.y | 0x00080000u);
  if (IsNanF64(d.x, d.y)) return uint2(d.x, d.y | 0x00080000u);
  if ((dzero && nzero) || (dinf && ninf)) return uint2(0u, 0xFFF80000u);
  if (dzero || ninf) return uint2(0u, sign | 0x7FF00000u);
  if (dinf || nzero) return uint2(0u, sign);
  if (int(nh >> 20) - int(dh >> 20) < -1075) return uint2(0u, sign);
  return IsNanF64(q.x, q.y) ? uint2(0u, sign | 0x7FF00000u) : uint2(q.x, sign | (q.y & 0x7FFFFFFFu));
}
"""


def _f32(code, index, literal, abs_bits, neg_bits):
    operand = source(code, literal)
    if abs_bits >> index & 1:
        operand = f"({operand} & 0x7FFFFFFFu)"
    if neg_bits >> index & 1:
        operand = f"({operand} ^ {SIGN})"
    return operand


def _f64(code, index, abs_bits, neg_bits):
    low, high = _words(code)
    if abs_bits >> index & 1:
        high = f"({high} & 0x7FFFFFFFu)"
    if neg_bits >> index & 1:
        high = f"({high} ^ {SIGN})"
    return f"uint2({low}, {high})"


def statements(name, fields, literal, active):
    """Instructions HLSL d'une instruction de division (encodage VOP3)."""
    if name not in TRANSLATED:
        raise Untranslatable(f"non traduite en HLSL : {name} ({UNTRANSLATED_REASONS[name]})")
    if fields.clamp or fields.omod:
        raise Untranslatable(f"non traduite en HLSL : {name} avec clamp/omod")
    vdst = fields.vdst
    if OPERATIONS[name].width == 64:
        q, d, n = (_f64(code, i, fields.abs, fields.neg) for i, code in enumerate(fields.sources))
        return [f"if ({active}) {{ uint2 r = DivFixupF64({q}, {d}, {n}); v[{vdst}] = r.x; v[{vdst + 1}] = r.y; }}"]
    abs_bits = 0 if name == "v_div_scale_f32" else fields.abs          # VOP3b : le champ abs porte SDST
    a, b, c = (_f32(code, i, literal, abs_bits, fields.neg) for i, code in enumerate(fields.sources))
    if name == "v_div_scale_f32":
        return [f"{{ bool o = false; if ({active}) v[{vdst}] = DivScaleF32({a}, {b}, {c}, o); "
                f"{scalar_target(fields.sdst)} = Ballot(lane, ({active}) && o); }}"]
    if name == "v_div_fmas_f32":
        return [f"if ({active}) v[{vdst}] = DivFmasF32({a}, {b}, {c}, ((s[106] >> lane) & 1u) != 0u);"]
    return [f"if ({active}) v[{vdst}] = DivFixupF32({a}, {b}, {c});"]
