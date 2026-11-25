"""Traduction HLSL de v_msad_u8, v_qsad_pk_u16_u8, v_mqsad_pk_u16_u8 et v_mqsad_u32_u8 (sémantique d'emu/rdna2_exec_sad.py).

La fenêtre k de S0 (64 bits) est AlignBit(haut, bas, 8k), qui traite le décalage nul à part (un décalage
HLSL de 32 bits n'est pas défini). Les mêmes formes que dans l'interpréteur sont refusées en les nommant.
Le texte ne lit ni EXEC bit à bit ni une autre ligne : il sert tel quel en wave64 (condition `LANE_ON`).
"""
from emu import rdna2_exec_sad
from emu.rdna2_hlsl_ops import Untranslatable, source, source_high

TRANSLATED = frozenset(rdna2_exec_sad.OPERATIONS)
VGPR_FIRST = 256

HELPERS = """uint MsadU8(uint a, uint b) {
  // v_msad_u8 : octets où la référence b vaut 0 exclus de la somme.
  uint r = 0u;
  // [loop] et non [unroll] : déroulée, cette boucle est mal compilée par le pilote AMD RDNA 2 (#151).
  [loop] for (uint i = 0u; i < 32u; i += 8u) {
    uint x = (a >> i) & 0xFFu, y = (b >> i) & 0xFFu;
    if (y != 0u) r += x > y ? x - y : y - x;
  }
  return r;
}
uint SadWindow(uint lo, uint hi, uint k, uint b, bool masked) {
  uint w = AlignBit(hi, lo, 8u * k);              // S0[8k+31:8k]
  return masked ? MsadU8(w, b) : SadU8(w, b);
}
uint2 QsadPk(uint lo, uint hi, uint b, uint2 c, bool masked) {
  uint r0 = (SadWindow(lo, hi, 0u, b, masked) + (c.x & 0xFFFFu)) & 0xFFFFu;
  uint r1 = (SadWindow(lo, hi, 1u, b, masked) + (c.x >> 16)) & 0xFFFFu;
  uint r2 = (SadWindow(lo, hi, 2u, b, masked) + (c.y & 0xFFFFu)) & 0xFFFFu;
  uint r3 = (SadWindow(lo, hi, 3u, b, masked) + (c.y >> 16)) & 0xFFFFu;
  return uint2(r0 | (r1 << 16), r2 | (r3 << 16));
}
uint4 MqsadU32(uint lo, uint hi, uint b, uint4 c) {
  return uint4(SadWindow(lo, hi, 0u, b, true) + c.x, SadWindow(lo, hi, 1u, b, true) + c.y,
               SadWindow(lo, hi, 2u, b, true) + c.z, SadWindow(lo, hi, 3u, b, true) + c.w);
}
"""


def statements(name, fields, literal, active):
    """Lignes HLSL d'une instruction de TRANSLATED ; `fields` : emu.rdna2_exec_vop3.Fields."""
    message = rdna2_exec_sad.refusal(name, fields)
    if message:
        raise Untranslatable(f"non traduite en HLSL : {message}")
    src0, src1, src2 = fields.sources
    d, b = fields.vdst, source(src1, literal)
    if name == "v_msad_u8":
        return [f"if ({active}) v[{d}] = MsadU8({source(src0, literal)}, {b}) + {source(src2, literal)};"]
    lo, hi = source(src0), source_high(src0)
    if name == "v_mqsad_u32_u8":
        c = ", ".join(f"v[{src2 - VGPR_FIRST + i}]" for i in range(4))
        return [f"{{ uint4 r = MqsadU32({lo}, {hi}, {b}, uint4({c})); "
                f"if ({active}) {{ v[{d}] = r.x; v[{d + 1}] = r.y; v[{d + 2}] = r.z; v[{d + 3}] = r.w; }} }}"]
    masked = "true" if name == "v_mqsad_pk_u16_u8" else "false"
    return [f"{{ uint2 r = QsadPk({lo}, {hi}, {b}, uint2({source(src2)}, {source_high(src2)}), {masked}); "
            f"if ({active}) {{ v[{d}] = r.x; v[{d + 1}] = r.y; }} }}"]
