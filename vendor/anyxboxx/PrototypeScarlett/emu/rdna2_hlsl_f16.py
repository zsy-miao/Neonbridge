"""Demi-précision (f16) dans le recompilateur HLSL, avec la sémantique de emu/rdna2_exec_f16.py.

Pas de types 16 bits (shader model 6.0, ni `half` ni f16tof32) : une valeur f16 est une moitié de VGPR (uint).
Chaque opération passe par le f32 dans une fonction `precise` (pas de x - x = 0 ni de 0 × x = 0), puis revient
en f16 par des routines entières, arrondi au plus proche pair. Somme, différence et produit de deux f16 sont
exacts en f32 ou n'y subissent qu'un arrondi sans effet sur le f16 (24 ≥ 2 × 11 + 2) ; le FMA arrondit d'abord
à l'impair (erreur exacte par TwoSum), puis en f16. Le pilote AMD (RX 9070 XT) replie pourtant `fadd x, 0.0`
en x, sans drapeau fast-math dans le DXIL : le signe d'un résultat exactement nul est donc fixé en entier.
"""
from emu.rdna2_exec_f16 import EXTENSIONS, INLINE_HALVES, OPERATIONS, VGPR_FIRST, WORD_RESULT
from emu.rdna2_hlsl_ops import Untranslatable, hexa, source

HELPERS = """uint F16ToF32(uint h) {
  uint s = (h & 0x8000u) << 16, e = (h >> 10) & 0x1Fu, m = h & 0x3FFu;
  if (e == 31u) return m != 0u ? 0x7FC00000u : s | 0x7F800000u;
  if (e != 0u) return s | ((e + 112u) << 23) | (m << 13);
  if (m == 0u) return s;
  uint p = firstbithigh(m);                       // dénormal f16 : normal en f32
  return s | ((p + 103u) << 23) | ((m << (23u - p)) & 0x7FFFFFu);
}
uint F32ToF16(uint f) {
  uint s = (f >> 16) & 0x8000u, a = f & 0x7FFFFFFFu;
  if (a > 0x7F800000u) return 0x7E00u;
  if (a >= 0x477FF000u) return s | 0x7C00u;
  if (a >= 0x38800000u) return s | ((a - 0x38000000u + 0xFFFu + ((a >> 13) & 1u)) >> 13);
  if (a < 0x33000000u) return s;
  uint shift = 126u - (a >> 23), m = (a & 0x7FFFFFu) | 0x800000u, q = m >> shift;
  uint rest = m & ((1u << shift) - 1u), tie = 1u << (shift - 1u);
  return s | (q + ((rest > tie || (rest == tie && (q & 1u) != 0u)) ? 1u : 0u));
}
uint F16Add(uint a, uint b) {
  precise float r = asfloat(F16ToF32(a)) + asfloat(F16ToF32(b));
  uint h = F32ToF16(asuint(r));
  return (h & 0x7FFFu) == 0u ? a & b & 0x8000u : h;    // somme exactement nulle : -0 seulement pour -0 + -0
}
uint F16Sub(uint a, uint b) { return F16Add(a, b ^ 0x8000u); }
uint F16Mul(uint a, uint b) {
  precise float r = asfloat(F16ToF32(a)) * asfloat(F16ToF32(b));
  return F32ToF16(asuint(r));
}
uint F16Fma(uint a, uint b, uint c) {
  precise float x = asfloat(F16ToF32(a)), y = asfloat(F16ToF32(b)), z = asfloat(F16ToF32(c));
  precise float p = x * y;                        // exact : deux mantisses de 11 bits
  precise float s = p + z;
  if ((asuint(s) & 0x7FFFFFFFu) == 0u) return (a ^ b) & c & 0x8000u;    // résultat exactement nul
  precise float t = s - p;
  precise float e = (p - (s - t)) + (z - t);      // erreur exacte de s (TwoSum)
  uint r = asuint(s);
  if ((r & 0x7F800000u) != 0x7F800000u && e != 0.0f && (r & 1u) == 0u)
    r += ((e > 0.0f) == (s > 0.0f)) ? 1u : 0xFFFFFFFFu;    // arrondi à l'impair
  return F32ToF16(r);
}
uint F16Ldexp(uint a, uint n) {
  int k = clamp(asint(n << 16) >> 16, -64, 64);   // au-delà de ±64, le f16 vaut déjà 0 ou l'infini
  precise float r = asfloat(F16ToF32(a)) * asfloat(uint(k + 127) << 23);
  return F32ToF16(asuint(r));
}
bool F16IsNan(uint h) { return (h & 0x7FFFu) > 0x7C00u; }
uint F16Key(uint h) { return (h & 0x8000u) != 0u ? ~h & 0xFFFFu : h | 0x8000u; }   // -0 juste sous +0
uint F16Min(uint a, uint b) {
  if (F16IsNan(a)) return F16IsNan(b) ? 0x7E00u : b;
  if (F16IsNan(b)) return a;
  return F16Key(a) <= F16Key(b) ? a : b;
}
uint F16Max(uint a, uint b) {
  if (F16IsNan(a)) return F16IsNan(b) ? 0x7E00u : b;
  if (F16IsNan(b)) return a;
  return F16Key(a) >= F16Key(b) ? a : b;
}
uint F16Clamp(uint h) { return F16IsNan(h) || (h & 0x8000u) != 0u ? 0u : min(h, 0x3C00u); }
"""

# Expression de chaque opération : {a} {b} {c} sources, {d} moitié basse de vdst avant l'instruction, {k} littéral.
TEMPLATES = {
    "v_add_f16": "F16Add({a}, {b})", "v_sub_f16": "F16Sub({a}, {b})", "v_subrev_f16": "F16Sub({b}, {a})",
    "v_mul_f16": "F16Mul({a}, {b})", "v_fmac_f16": "F16Fma({a}, {b}, {d})", "v_fmamk_f16": "F16Fma({a}, {k}, {b})",
    "v_fmaak_f16": "F16Fma({a}, {b}, {k})", "v_fma_f16": "F16Fma({a}, {b}, {c})", "v_max_f16": "F16Max({a}, {b})",
    "v_min_f16": "F16Min({a}, {b})", "v_ldexp_f16": "F16Ldexp({a}, {b})", "v_cvt_f16_f32": "F32ToF16({a})",
    "v_cvt_f32_f16": "F16ToF32({a})",
}


def _operand(name, kind, code, literal, high=False):
    """Source selon son type, comme l'interpréteur : mot f32 entier, ou 16 bits (moitié, constante, littéral)."""
    if kind == "w":
        return source(code, literal)
    if code < 128 or code >= VGPR_FIRST:                 # SGPR, VCC, M0, EXEC, NULL et VGPR
        return f"({source(code, literal)} >> 16)" if high else f"({source(code, literal)} & 0xFFFFu)"
    if high:
        raise Untranslatable(f"{name} : op_sel sur la moitié haute d'une constante non pris en charge")
    if code in INLINE_HALVES:
        if kind == "i":
            raise Untranslatable(f"{name} : constante flottante sur une source entière non prise en charge")
        return hexa(INLINE_HALVES[code])
    return f"({source(code, literal)} & 0xFFFFu)"         # constante entière ou littéral : 16 bits bas


def _written(name, d, expression, high):
    """Nouvelle valeur de v[d] : le résultat dans une moitié, l'autre gardée ; 32 bits pour v_cvt_f32_f16."""
    if name in WORD_RESULT:
        return expression
    if high:
        return f"(v[{d}] & 0xFFFFu) | ({expression} << 16)"
    return f"(v[{d}] & 0xFFFF0000u) | {expression}"


def vector(name, fields, literal, active):
    """VOP1/VOP2 f16 : sources et résultat dans la moitié basse."""
    if fields["src0"] in EXTENSIONS:
        raise Untranslatable(f"{name} : forme SDWA/DPP (src0 = 0x{fields['src0']:X}) non prise en charge en f16")
    kinds = OPERATIONS[name][0]
    codes = (fields["src0"], VGPR_FIRST + fields["vsrc1"]) if "vsrc1" in fields else (fields["src0"],)
    a, b = ([_operand(name, kind, code, literal) for kind, code in zip(kinds, codes)] + ["0u"])[:2]
    d = fields["vdst"]
    expression = TEMPLATES[name].format(a=a, b=b, c="0u", d=f"(v[{d}] & 0xFFFFu)", k=hexa((literal or 0) & 0xFFFF))
    return [f"if ({active}) v[{d}] = {_written(name, d, expression, False)};"]


def _check_vop3(name, kinds, fields, literal):
    """Formes VOP3 dont le comportement n'est pas sourcé : refusées comme dans l'interpréteur."""
    if name in WORD_RESULT and (fields.clamp or fields.omod):
        raise Untranslatable(f"{name} : clamp ou omod sur un résultat f32 non pris en charge")
    if fields.omod:
        raise Untranslatable(f"{name} : omod sur un résultat f16 non pris en charge")
    if fields.op_sel and name == "v_fmac_f16":
        raise Untranslatable(f"{name} : op_sel non pris en charge en VOP3")
    if fields.op_sel >> 3 & 1 and name in WORD_RESULT:
        raise Untranslatable(f"{name} : op_sel sur une destination de 32 bits non pris en charge")
    if name in ("v_fmamk_f16", "v_fmaak_f16") and literal is None:
        raise Untranslatable(f"{name} (VOP3) : sans littéral, la constante K n'existe pas")
    for index, kind in enumerate(kinds):
        if kind == "w" and fields.op_sel >> index & 1:
            raise Untranslatable(f"{name} : op_sel sur une source de 32 bits non pris en charge")
        if kind == "i" and (fields.abs | fields.neg) >> index & 1:
            raise Untranslatable(f"{name} : abs/neg sur une source entière non pris en charge")


def _modified(operand, kind, fields, index):
    """abs et neg de VOP3 : bit de signe de la moitié f16 (bit 15) ou du mot f32 (bit 31)."""
    if kind == "i":
        return operand
    if fields.abs >> index & 1:
        operand = f"({operand} & {'0x7FFFFFFFu' if kind == 'w' else '0x7FFFu'})"
    if fields.neg >> index & 1:
        operand = f"({operand} ^ {'0x80000000u' if kind == 'w' else '0x8000u'})"
    return operand


def vop3(name, fields, literal, active):
    """VOP3 f16 : op_sel (bits 0 à 2) lit la moitié haute d'une source, op_sel bit 3 écrit le résultat dans la
    moitié haute de vdst et garde la basse ; clamp borne le résultat f16 à [0, 1]."""
    kinds = OPERATIONS[name][0]
    _check_vop3(name, kinds, fields, literal)
    a, b, c = ([_modified(_operand(name, kind, code, literal, fields.op_sel >> index & 1), kind, fields, index)
                for index, (kind, code) in enumerate(zip(kinds, fields.sources))] + ["0u", "0u"])[:3]
    d = fields.vdst
    expression = TEMPLATES[name].format(a=a, b=b, c=c, d=f"(v[{d}] & 0xFFFFu)", k=hexa((literal or 0) & 0xFFFF))
    if fields.clamp:
        expression = f"F16Clamp({expression})"
    return [f"if ({active}) v[{d}] = {_written(name, d, expression, fields.op_sel >> 3 & 1)};"]
