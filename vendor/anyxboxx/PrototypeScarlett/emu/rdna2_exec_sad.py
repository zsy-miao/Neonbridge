"""SAD masqués et quadruples VOP3 (v_msad_u8, v_qsad_pk_u16_u8, v_mqsad_pk_u16_u8, v_mqsad_u32_u8) de l'interpréteur.

- v_msad_u8 : D = S2 + Σ |S0[i] - S1[i]| sur les quatre octets, sauf ceux où l'octet de référence S1[i] est nul.
- v_qsad_pk_u16_u8 et v_mqsad_pk_u16_u8 : S0 (64 bits) donne quatre fenêtres de 32 bits, S0[31:0], S0[39:8],
  S0[47:16] et S0[55:24] ; la k-ième vaut v_sad_u8 (ou v_msad_u8) de la fenêtre, de S1 et de S2[16k+15:16k],
  tronquée à 16 bits et rangée dans D[16k+15:16k] (D et S2 sur 64 bits).
- v_mqsad_u32_u8 : mêmes fenêtres, accumulateurs et résultats de 32 bits (S2 et D sur quatre VGPR).

Sources : descriptions et largeurs d'opérandes de la spécification XML d'AMD (amdgpu_isa_rdna2.xml : S0 B64,
S2 B64 ou B128 en VGPR, D B64 ou B128) et pseudo-code du guide RDNA 3 d'AMD (mêmes instructions,
`tmp[63:48] = 16'B(v_sad_u8(S0[55:24], S1[31:0], S2[63:48].u32))`…). Les modificateurs VOP3 (abs, neg,
clamp, omod) n'ont pas de sens décrit pour ces entiers : ils lèvent Untranslated en les nommant.
"""
from emu.rdna2_exec_cmp64 import read64
from emu.rdna2_exec_ops import MASK32

VGPR_FIRST, BYTE = 256, 0xFF
INLINE_INTEGERS_END = 209                       # SGPR (0-127) et entiers en ligne (128-208) : lus sur 64 bits
WINDOWS = 4                                     # fenêtres de S0 décalées de 0, 8, 16 et 24 bits
# nom : (SAD masqué, bits par accumulateur et par résultat ; None pour v_msad_u8)
OPERATIONS = {
    "v_msad_u8": (True, None),
    "v_qsad_pk_u16_u8": (False, 16),
    "v_mqsad_pk_u16_u8": (True, 16),
    "v_mqsad_u32_u8": (True, 32),
}


def sad(a, b, masked):
    """Σ |a[i] - b[i]| sur quatre octets ; `masked` : sans les octets où b[i] (la référence) vaut 0."""
    total = 0
    for shift in (0, 8, 16, 24):
        x, y = a >> shift & BYTE, b >> shift & BYTE
        if not (masked and y == 0):
            total += abs(x - y)
    return total


def quad(name, s0, s1, s2):
    """Les quatre résultats empaquetés de v_qsad/v_mqsad (entier de 64 ou 128 bits)."""
    masked, width = OPERATIONS[name]
    mask = (1 << width) - 1
    result = 0
    for k in range(WINDOWS):
        window = s0 >> (8 * k) & MASK32
        accumulator = s2 >> (width * k) & mask
        result |= ((sad(window, s1, masked) + accumulator) & mask) << (width * k)
    return result


def refusal(name, fields):
    """Raison de ne pas exécuter (ni traduire) cette forme, ou None.

    Refusés : les modificateurs VOP3 (abs, neg, clamp, omod), dont le sens n'est pas décrit pour ces
    entiers ; S2 de v_mqsad_u32_u8 hors VGPR (la spécification impose un VGPR) ; un littéral ou une
    constante flottante sur une source de 64 bits, dont l'extension n'est pas vérifiée."""
    present = [label for label, value in (("abs", fields.abs), ("neg", fields.neg), ("clamp", fields.clamp),
                                          ("omod", fields.omod)) if value]
    if present:
        return f"{name} avec {', '.join(present)} : sens non documenté"
    src0, _, src2 = fields.sources
    if name == "v_mqsad_u32_u8" and src2 < VGPR_FIRST:
        return f"{name} : S2 (128 bits) doit être un VGPR"
    wide = () if name == "v_msad_u8" else (src0, src2)
    if any(not (code >= VGPR_FIRST or code < INLINE_INTEGERS_END) for code in wide):
        return f"{name} : littéral ou constante flottante sur une source de 64 bits, extension non vérifiée"
    return None


def _wide_source(wave, code, lane, words):
    """S2 de 64 bits (paire de registres ou constante entière en ligne) ou de 128 bits (quatre VGPR)."""
    if code >= VGPR_FIRST:
        return sum(wave.v[code - VGPR_FIRST + i][lane] << (32 * i) for i in range(words))
    return read64(wave, code, lane, False)


def execute(wave, name, fields, literal):
    """Exécute une instruction de OPERATIONS ; `fields` : emu.rdna2_exec_vop3.Fields."""
    message = refusal(name, fields)
    if message:
        from emu.rdna2_exec import Untranslated
        raise Untranslated(message)
    src0, src1, src2 = fields.sources
    words = 4 if name == "v_mqsad_u32_u8" else 2
    for lane in wave.active():
        b = wave.read_lane(src1, lane, literal)
        if name == "v_msad_u8":
            a, c = wave.read_lane(src0, lane, literal), wave.read_lane(src2, lane, literal)
            wave.v[fields.vdst][lane] = (sad(a, b, True) + c) & MASK32
            continue
        result = quad(name, read64(wave, src0, lane, False), b, _wide_source(wave, src2, lane, words))
        for i in range(words):
            wave.v[fields.vdst + i][lane] = result >> (32 * i) & MASK32
