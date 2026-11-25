"""Texte de l'offset de ds_swizzle_b32 au format de LLVM (« swizzle(MODE,...) »).

Port de AMDGPUInstPrinter::printSwizzle et des constantes Swizzle de SIDefines.h
(llvm/llvm-project, Apache 2.0 avec exception LLVM). RDNA 2 (gfx10) a les modes FFT et ROTATE.
"""
QUAD_PERM_ENC, QUAD_PERM_ENC_MASK = 0x8000, 0xFF00
BITMASK_PERM_ENC, BITMASK_PERM_ENC_MASK = 0x0000, 0x8000
ROTATE_MODE_LO, FFT_MODE_LO = 0xC000, 0xE000
LANE_MASK, LANE_SHIFT, LANE_NUM = 0x3, 2, 4
BITMASK_MASK = BITMASK_MAX = 0x1F
BITMASK_WIDTH = 5
BITMASK_AND_SHIFT, BITMASK_OR_SHIFT, BITMASK_XOR_SHIFT = 0, 5, 10
FFT_SWIZZLE_MASK = 0x1F
ROTATE_DIR_SHIFT, ROTATE_DIR_MASK = 10, 0x1
ROTATE_SIZE_SHIFT, ROTATE_SIZE_MASK = 5, 0x1F
U16 = 0xFFFF


def _power_of_two(value):
    return value > 0 and value & (value - 1) == 0


def _bitmask_text(and_mask, or_mask, xor_mask):
    """Chaîne de 5 caractères : 0, 1, p (lane conservée) ou i (lane inversée), bit 4 d'abord."""
    probe0 = (0 & and_mask | or_mask) ^ xor_mask
    probe1 = (BITMASK_MASK & and_mask | or_mask) ^ xor_mask
    chars = []
    for bit in reversed(range(BITMASK_WIDTH)):
        p0, p1 = probe0 >> bit & 1, probe1 >> bit & 1
        chars.append(("1" if p0 else "0") if p0 == p1 else ("i" if p0 else "p"))
    return '"' + "".join(chars) + '"'


def _rotate_or_fft(imm):
    if imm >= FFT_MODE_LO:
        return f"swizzle(FFT,{imm & FFT_SWIZZLE_MASK})"
    direction = (imm >> ROTATE_DIR_SHIFT) & ROTATE_DIR_MASK
    return f"swizzle(ROTATE,{direction},{(imm >> ROTATE_SIZE_SHIFT) & ROTATE_SIZE_MASK})"


def _bitmask_mode(imm):
    and_mask = (imm >> BITMASK_AND_SHIFT) & BITMASK_MASK
    or_mask = (imm >> BITMASK_OR_SHIFT) & BITMASK_MASK
    xor_mask = (imm >> BITMASK_XOR_SHIFT) & BITMASK_MASK
    if and_mask == BITMASK_MAX and or_mask == 0 and bin(xor_mask).count("1") == 1:
        return f"swizzle(SWAP,{xor_mask})"
    if and_mask == BITMASK_MAX and or_mask == 0 and xor_mask > 0 and _power_of_two(xor_mask + 1):
        return f"swizzle(REVERSE,{xor_mask + 1})"
    group = BITMASK_MAX - and_mask + 1
    if group > 1 and _power_of_two(group) and or_mask < group and xor_mask == 0:
        return f"swizzle(BROADCAST,{group},{or_mask})"
    return f"swizzle(BITMASK_PERM,{_bitmask_text(and_mask, or_mask, xor_mask)})"


def text(imm):
    """Valeur écrite après « offset: » ; chaîne vide pour 0 (LLVM n'écrit alors pas d'offset)."""
    imm &= U16
    if imm == 0:
        return ""
    if imm >= ROTATE_MODE_LO:
        return _rotate_or_fft(imm)
    if imm & QUAD_PERM_ENC_MASK == QUAD_PERM_ENC:
        lanes = [(imm >> (LANE_SHIFT * lane)) & LANE_MASK for lane in range(LANE_NUM)]
        return "swizzle(QUAD_PERM," + ",".join(map(str, lanes)) + ")"
    if imm & BITMASK_PERM_ENC_MASK == BITMASK_PERM_ENC:
        return _bitmask_mode(imm)
    return str(imm)
