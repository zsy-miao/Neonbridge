"""Texte des formes SDWA, DPP16 et DPP8 des VOP1, VOP2 et VOPC au format de LLVM (gfx10).

Le src0 du premier mot (0xF9 SDWA, 0xFA DPP16, 0xE9/0xEA DPP8, 0xEA avec fi) annonce un second mot :
- SDWA : src0 (7-0), dst_sel (10-8), dst_unused (12-11), clamp (13), omod (15-14), src0_sel (18-16),
  sext (19), neg (20), abs (21), src0 scalaire (23), src1_sel (26-24), sext (27), neg (28), abs (29),
  src1 scalaire (31). En VOPC, les bits 14-8 portent sdst et le bit 15 dit s'il remplace vcc.
- DPP16 : src0 (7-0), dpp_ctrl (16-8), fi (18), bound_ctrl (19), neg/abs de src0 (20, 21) et de
  src1 (22, 23), bank_mask (27-24), row_mask (31-28).
- DPP8 : src0 (7-0), puis huit sélecteurs de 3 bits.

D'après AMDGPUInstPrinter (printSDWASel, printSDWADstUnused, printDPPCtrl, printDPP8) et
VOPInstructions.td (llvm/llvm-project, Apache 2.0 avec exception LLVM).
"""
DWORD = 4
SDWA, DPP16, DPP8, DPP8_FI = 0xF9, 0xFA, 0xE9, 0xEA
BYTE, VGPR_FIRST = 0xFF, 256
SELECTIONS = ("BYTE_0", "BYTE_1", "BYTE_2", "BYTE_3", "WORD_0", "WORD_1", "DWORD")
UNUSED = ("UNUSED_PAD", "UNUSED_SEXT", "UNUSED_PRESERVE")
# (bit sext, bit neg, bit abs, bit « scalaire ») de src0 et src1 dans le mot SDWA ; neg/abs en DPP16.
SDWA_SOURCE_BITS = ((19, 20, 21, 23), (27, 28, 29, 31))
DPP_SOURCE_BITS = ((20, 21), (22, 23))
VOPC_SDST_SHIFT, VOPC_SD_BIT = 8, 15
CLAMP_BIT, OMOD_SHIFT = 13, 14
FI_BIT, BOUND_CTRL_BIT = 18, 19
DPP8_SELECTORS, DPP8_BITS = 8, 3
# dpp_ctrl gfx10 : (premier, dernier, texte) ; les autres valeurs sont réservées.
DPP_ROWS = ((0x101, 0x10F, "row_shl:{}"), (0x111, 0x11F, "row_shr:{}"), (0x121, 0x12F, "row_ror:{}"),
            (0x140, 0x140, "row_mirror"), (0x141, 0x141, "row_half_mirror"),
            (0x150, 0x15F, "row_share:{}"), (0x160, 0x16F, "row_xmask:{}"))
QUAD_PERM_LAST = 0xFF


def dpp_control(ctrl):
    """Texte de dpp_ctrl (quad_perm, row_*), ou None si la valeur n'existe pas en gfx10."""
    if ctrl <= QUAD_PERM_LAST:
        return f"quad_perm:[{','.join(str(ctrl >> (2 * lane) & 3) for lane in range(4))}]"
    for first, last, pattern in DPP_ROWS:
        if first <= ctrl <= last:
            return pattern.format(ctrl & 0xF)
    return None


def _decorate(text, extension, bits):
    sext, neg, absolute = (extension >> bit & 1 if bit is not None else 0 for bit in bits)
    if absolute:
        text = f"|{text}|"
    if neg:
        text = f"-{text}"
    return f"sext({text})" if sext else text


def _source(kind, extension, word, index, width):
    """Texte de src0 (index 0) ou src1 (index 1) avec ses modificateurs."""
    from emu.rdna2_operands import source

    if index == 0:
        code = extension & BYTE
        scalar = kind == SDWA and extension >> SDWA_SOURCE_BITS[0][3] & 1
    else:
        code = (word >> 9) & BYTE
        scalar = kind == SDWA and extension >> SDWA_SOURCE_BITS[1][3] & 1
    text = source(code if scalar else VGPR_FIRST + code, width)
    if kind == SDWA:
        return _decorate(text, extension, SDWA_SOURCE_BITS[index][:3])
    if kind == DPP16:
        return _decorate(text, extension, (None, *DPP_SOURCE_BITS[index]))
    return text


def _operands(family, word, extension, kind, shape):
    from emu.rdna2_operands import render, source

    parts = []
    for position, element in enumerate(shape):
        name, option = element
        if name == "src0":
            parts.append(_source(kind, extension, word, 0, option))
        elif name == "vsrc1":
            parts.append(_source(kind, extension, word, 1, option))
        elif family == "VOPC" and position == 0 and name == "=" and kind == SDWA and extension >> VOPC_SD_BIT & 1:
            parts.append(source((extension >> VOPC_SDST_SHIFT) & 0x7F))
        else:
            parts.append(render(family, word, None, element))
    return parts


def _sdwa_modifiers(family, extension, has_src1):
    from emu.rdna2_operands import OUTPUT_MODIFIERS

    parts = []
    if family != "VOPC":
        if extension >> CLAMP_BIT & 1:
            parts.append("clamp")
        if (extension >> OMOD_SHIFT) & 3:
            parts.append(OUTPUT_MODIFIERS[(extension >> OMOD_SHIFT) & 3])
        parts += [f"dst_sel:{SELECTIONS[(extension >> 8) & 7]}", f"dst_unused:{UNUSED[(extension >> 11) & 3]}"]
    parts.append(f"src0_sel:{SELECTIONS[(extension >> 16) & 7]}")
    if has_src1:
        parts.append(f"src1_sel:{SELECTIONS[(extension >> 24) & 7]}")
    return parts


def _dpp16_modifiers(extension):
    control = dpp_control((extension >> 8) & 0x1FF)
    if control is None:
        return None
    parts = [control, f"row_mask:0x{extension >> 28 & 0xF:x}", f"bank_mask:0x{extension >> 24 & 0xF:x}"]
    if extension >> BOUND_CTRL_BIT & 1:
        parts.append("bound_ctrl:1")
    if extension >> FI_BIT & 1:
        parts.append("fi:1")
    return parts


def _dpp8_modifiers(extension, kind):
    lanes = [(extension >> (8 + DPP8_BITS * lane)) & 7 for lane in range(DPP8_SELECTORS)]
    parts = [f"dpp8:[{','.join(map(str, lanes))}]"]
    return parts + ["fi:1"] if kind == DPP8_FI else parts


def _modifiers(family, kind, extension, shape):
    if kind == SDWA:
        names = [name for name, _ in shape]
        return _sdwa_modifiers(family, extension, "vsrc1" in names)
    if kind == DPP16:
        return _dpp16_modifiers(extension)
    return _dpp8_modifiers(extension, kind)


def text(family, name, data, offset, shape):
    """Instruction SDWA/DPP désassemblée, ou None (mot de contrôle absent, valeur réservée, VOPC en DPP)."""
    if len(data) < offset + 2 * DWORD:
        return None
    word = int.from_bytes(data[offset:offset + DWORD], "little")
    extension = int.from_bytes(data[offset + DWORD:offset + 2 * DWORD], "little")
    kind = word & 0x1FF
    if kind != SDWA and family == "VOPC":            # gfx10 n'a pas de VOPC en DPP
        return None
    modifiers = _modifiers(family, kind, extension, shape)
    if modifiers is None:
        return None
    suffix = "_sdwa" if kind == SDWA else "_dpp"
    operands = ", ".join(_operands(family, word, extension, kind, shape))
    return " ".join(part for part in (f"{name}{suffix}", operands, *modifiers) if part)
