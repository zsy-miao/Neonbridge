"""Traduction HLSL des produits scalaires entiers VOP3P (v_dot2/v_dot4/v_dot8), sémantique d'emu/rdna2_exec_dot.py.

Tout le calcul se fait en `uint` : le produit de deux valeurs étendues en signe, pris modulo 2^32, est celui
des entiers signés, et l'addition non signée déborde sans comportement indéfini. Les formes avec op_sel,
op_sel_hi, neg_lo, neg_hi ou clamp hors défaut lèvent Untranslatable en les nommant, comme l'interpréteur.
Le texte ne lit ni EXEC bit à bit ni une autre ligne : il sert tel quel en wave64 (condition `LANE_ON`).
"""
from emu import rdna2_exec_dot
from emu.rdna2_hlsl_ops import Untranslatable, source
from emu.rdna2_operands_vop3p import SOURCE_MASK, SOURCE_SHIFTS, VDST_MASK

TRANSLATED = frozenset(rdna2_exec_dot.OPERATIONS)

HELPERS = """uint DotPacked(uint a, uint b, uint c, uint width, bool sgn) {
  // v_dot2/4/8 : c + somme des produits des éléments de `width` bits, modulo 2^32 (calcul non signé).
  uint r = c;
  [unroll] for (uint i = 0u; i < 32u; i += width) {
    uint x = (a >> i) & ((1u << width) - 1u), y = (b >> i) & ((1u << width) - 1u);
    if (sgn) {
      x = asuint(asint(a << (32u - width - i)) >> (32u - width));
      y = asuint(asint(b << (32u - width - i)) >> (32u - width));
    }
    r += x * y;
  }
  return r;
}
"""


def statements(name, word, literal, active):
    """Ligne HLSL d'un produit scalaire VOP3P ; `word` : les deux mots de l'instruction (entier 64 bits)."""
    if name not in TRANSLATED:
        raise Untranslatable(f"non traduite en HLSL : {name} (VOP3P)")
    modifiers = rdna2_exec_dot.unsupported_modifiers(word)
    if modifiers:
        raise Untranslatable(f"non traduite en HLSL : {name} avec {', '.join(modifiers)}")
    width, is_signed = rdna2_exec_dot.OPERATIONS[name]
    a, b, c = (source(word >> shift & SOURCE_MASK, literal) for shift in SOURCE_SHIFTS)
    sign = "true" if is_signed else "false"
    return [f"if ({active}) v[{word & VDST_MASK}] = DotPacked({a}, {b}, {c}, {width}u, {sign});"]
