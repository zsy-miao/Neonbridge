"""Texte des exports RDNA 2 (EXP) au format de LLVM : « exp mrt0, v1, off, v3, off done vm ».

Mot 0 : en (bits 3-0), cible (9-4), compr (10), done (11), vm (12) ; mot 1 : vsrc0..vsrc3.
D'après AMDGPUInstPrinter::printExpSrcN/printExpTgt, Exp::getTgtName et la cible gfx10 de
EXPInstructions.td (llvm/llvm-project, Apache 2.0 avec exception LLVM).
"""
DWORD = 4
TARGET_SHIFT, TARGET_MASK = 4, 0x3F
COMPR_BIT, DONE_BIT, VM_BIT = 10, 11, 12
CHANNELS = 4
# (nom, première cible, dernier indice) ; indice 0 seul : pas de numéro après le nom.
TARGETS = (("null", 9, 0), ("mrtz", 8, 0), ("prim", 20, 0), ("mrt", 0, 7), ("pos", 12, 4),
           ("param", 32, 31))


def target_name(target):
    """Nom de la cible en gfx10, ou « invalid_target_N » (comme LLVM)."""
    for name, first, last_index in TARGETS:
        if first <= target <= first + last_index:
            return name if last_index == 0 else f"{name}{target - first}"
    return f"invalid_target_{target}"


def _sources(word):
    registers = [(word >> (32 + 8 * index)) & 0xFF for index in range(CHANNELS)]
    if word >> COMPR_BIT & 1:                      # compr : src0, src0, src1, src1
        registers = [registers[index // 2] for index in range(CHANNELS)]
    return [f"v{register}" if word >> index & 1 else "off" for index, register in enumerate(registers)]


VINTRP_NAMES = {0: "v_interp_p1_f32_e32", 1: "v_interp_p2_f32_e32", 2: "v_interp_mov_f32_e32"}
MOV_SOURCES = {0: "p10", 1: "p20", 2: "p0"}
CHANNEL_NAMES = "xyzw"


def vintrp_text(data, offset=0):
    """Interpolation VINTRP au format de LLVM : « v_interp_p1_f32_e32 v2, v0, attr0.x »."""
    if len(data) < offset + DWORD:
        return None
    word = int.from_bytes(data[offset:offset + DWORD], "little")
    op, vdst, attr, chan, vsrc = (word >> 16) & 3, (word >> 18) & 0xFF, (word >> 10) & 0x3F, (word >> 8) & 3, word & 0xFF
    if op not in VINTRP_NAMES:
        return None
    source = MOV_SOURCES.get(vsrc, f"p{vsrc}") if op == 2 else f"v{vsrc}"
    return f"{VINTRP_NAMES[op]} v{vdst}, {source}, attr{attr}.{CHANNEL_NAMES[chan]}"


def text(data, offset=0):
    """Instruction EXP désassemblée, ou None si les deux mots ne sont pas disponibles."""
    if len(data) < offset + 2 * DWORD:
        return None
    word = int.from_bytes(data[offset:offset + 2 * DWORD], "little")
    target = target_name((word >> TARGET_SHIFT) & TARGET_MASK)
    flags = [name for bit, name in ((DONE_BIT, "done"), (COMPR_BIT, "compr"), (VM_BIT, "vm"))
             if word >> bit & 1]
    return " ".join([f"exp {target}, " + ", ".join(_sources(word)), *flags])
