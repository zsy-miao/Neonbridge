"""Produits scalaires entiers VOP3P (v_dot2/v_dot4/v_dot8) de l'interpréteur de référence RDNA 2.

D = S2 + Σ S0[i] × S1[i] sur 32 bits (reste modulo 2^32), où S0 et S1 sont lus comme 2 entiers de 16 bits,
4 de 8 bits ou 8 de 4 bits, signés (_i32_) ou non (_u32_). Sources :
- spécification lisible par machine d'AMD (amdgpu_isa_rdna2.xml) : descriptions et types d'opérandes
  (S0, S1 en FMT_NUM_PK2_I16, PK4_I8, PK8_I4…, S2 et D en FMT_NUM_I32 ou U32) ;
- pseudo-code du guide RDNA 3 d'AMD pour V_DOT4_U32_U8 et V_DOT8_U32_U4 (mêmes opcodes 0x17 et 0x19,
  `tmp = S2.u32; tmp += u8_to_u32(S0[7:0]) * u8_to_u32(S1[7:0]); …`) ;
- LLVM (IntrinsicsAMDGPU.td, llvm.amdgcn.sdot2/udot2/sdot4/udot4/sdot8/udot8 :
  « %r = %a[0] * %b[0] + … + %c ») ; aucun code copié.

Seule la forme sans modificateur est exécutée : op_sel à 0, op_sel_hi à 1 partout (l'encodage par
défaut de LLVM), ni neg_lo, ni neg_hi, ni clamp. Le sens de clamp (saturation) et des op_sel sur ces
produits n'est pas décrit pour RDNA 2 : une forme qui les utilise lève Untranslated en les nommant.
"""
from emu.rdna2_exec_ops import MASK32, signed
from emu.rdna2_operands_vop3p import (CLAMP_BIT, NEG_HI_SHIFT, NEG_LO_SHIFT, OP_SEL_HI_SHIFTS, OP_SEL_SHIFT,
                                      SOURCE_MASK, SOURCE_SHIFTS, VDST_MASK)

# nom : (largeur d'un élément en bits, signé)
OPERATIONS = {
    "v_dot2_i32_i16": (16, True), "v_dot2_u32_u16": (16, False),
    "v_dot4_i32_i8": (8, True), "v_dot4_u32_u8": (8, False),
    "v_dot8_i32_i4": (4, True), "v_dot8_u32_u4": (4, False),
}
THREE_BITS = 0x7


def supports(name):
    return name in OPERATIONS


def unsupported_modifiers(word):
    """Noms des modificateurs VOP3P présents hors de la forme par défaut (liste vide sinon)."""
    found = []
    if word >> OP_SEL_SHIFT & THREE_BITS:
        found.append("op_sel")
    if any(not word >> shift & 1 for shift in OP_SEL_HI_SHIFTS):
        found.append("op_sel_hi")
    if word >> NEG_LO_SHIFT & THREE_BITS:
        found.append("neg_lo")
    if word >> NEG_HI_SHIFT & THREE_BITS:
        found.append("neg_hi")
    if word >> CLAMP_BIT & 1:
        found.append("clamp")
    return found


def dot(name, a, b, c):
    """S2 + Σ S0[i] × S1[i], modulo 2^32."""
    width, is_signed = OPERATIONS[name]
    mask = (1 << width) - 1
    total = c
    for shift in range(0, 32, width):
        x, y = a >> shift & mask, b >> shift & mask
        if is_signed:
            x, y = signed(x, width), signed(y, width)
        total += x * y
    return total & MASK32


def execute(wave, name, word, literal):
    """Exécute un produit scalaire VOP3P ; `word` : les deux mots de l'instruction (entier 64 bits)."""
    modifiers = unsupported_modifiers(word)
    if modifiers:
        from emu.rdna2_exec import Untranslated
        raise Untranslated(f"{name} avec {', '.join(modifiers)} : sens non documenté pour RDNA 2")
    codes = [word >> shift & SOURCE_MASK for shift in SOURCE_SHIFTS]
    destination = wave.v[word & VDST_MASK]
    for lane in wave.active():
        a, b, c = (wave.read_lane(code, lane, literal) for code in codes)
        destination[lane] = dot(name, a, b, c)
