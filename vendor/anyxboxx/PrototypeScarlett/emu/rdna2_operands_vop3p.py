"""Texte des instructions VOP3P au format de LLVM : paquets 16 bits (v_pk_*), mix f16/f32, produits scalaires.

Deux mots lus comme un entier 64 bits : vdst (bits 7-0), neg_hi (10-8), op_sel (13-11), op_sel_hi
de src2 (14), clamp (15), opcode (22-16) ; src0, src1, src2 (40-32, 49-41, 58-50), op_sel_hi de src0
et src1 (60-59), neg_lo (63-61). Un littéral 32 bits suit quand une source vaut 255.

D'après AMDGPUInstPrinter::printPackedModifier et printOperandAndFPInputMods, et VOP3PInstructions.td
(llvm/llvm-project, Apache 2.0 avec exception LLVM). Les formes packed taisent op_sel_hi quand il vaut
1 partout ; les mix (v_fma_mix*) ne sont pas packed : op_sel_hi tu à 0 partout, neg_lo écrit « - » et
neg_hi « |…| » devant l'opérande, sans neg_lo:[…] ni neg_hi:[…].
"""
from emu import rdna2

DWORD = 4
TWO_WORDS = 2 * DWORD
LITERAL = 255
OPCODE_SHIFT, OPCODE_MASK = 16, 0x7F
VDST_MASK = 0xFF
SOURCE_SHIFTS, SOURCE_MASK = (32, 41, 50), 0x1FF
NEG_HI_SHIFT, OP_SEL_SHIFT, NEG_LO_SHIFT = 8, 11, 61
OP_SEL_HI_SHIFTS = (59, 60, 14)          # src0, src1 dans le second mot ; src2 dans le premier
CLAMP_BIT = 15
THREE_SOURCES = ("v_pk_mad_", "v_pk_fma_", "v_fma_mix", "v_dot")
MIX = "v_fma_mix"


def _bits(word, shifts):
    return [word >> shift & 1 for shift in shifts]


def _list(name, bits):
    return f"{name}:[{','.join(map(str, bits))}]"


def _literal(data, offset, codes):
    """(littéral ou None, lisible) : lisible est faux si une source l'annonce et qu'il manque."""
    if LITERAL not in codes:
        return None, True
    end = offset + TWO_WORDS
    if len(data) < end + DWORD:
        return None, False
    return int.from_bytes(data[end:end + DWORD], "little"), True


def _operand(text, neg, absolute):
    if absolute:
        text = f"|{text}|"
    return f"-{text}" if neg else text


def _modifiers(word, count, mix):
    op_sel = _bits(word, [OP_SEL_SHIFT + i for i in range(count)])
    op_sel_hi = _bits(word, OP_SEL_HI_SHIFTS[:count])
    neg_lo = _bits(word, [NEG_LO_SHIFT + i for i in range(count)])
    neg_hi = _bits(word, [NEG_HI_SHIFT + i for i in range(count)])
    parts = [_list("op_sel", op_sel)] if any(op_sel) else []
    default_hi = 0 if mix else 1
    if any(bit != default_hi for bit in op_sel_hi):
        parts.append(_list("op_sel_hi", op_sel_hi))
    if not mix:
        parts += [_list(name, bits) for name, bits in (("neg_lo", neg_lo), ("neg_hi", neg_hi)) if any(bits)]
    if word >> CLAMP_BIT & 1:
        parts.append("clamp")
    return parts


def text(data, offset=0):
    """Instruction VOP3P désassemblée, ou None (opcode sans nom, mots ou littéral manquants)."""
    from emu.rdna2_operands import source

    if len(data) < offset + TWO_WORDS:
        return None
    word = int.from_bytes(data[offset:offset + TWO_WORDS], "little")
    name = rdna2.mnemonic("VOP3P", (word >> OPCODE_SHIFT) & OPCODE_MASK)
    if name is None:
        return None
    count = 3 if name.startswith(THREE_SOURCES) else 2
    codes = [(word >> shift) & SOURCE_MASK for shift in SOURCE_SHIFTS[:count]]
    literal, readable = _literal(data, offset, codes)
    if not readable:
        return None
    mix = name.startswith(MIX)
    operands = [f"v{word & VDST_MASK}"]
    for index, code in enumerate(codes):
        neg = mix and word >> (NEG_LO_SHIFT + index) & 1
        absolute = mix and word >> (NEG_HI_SHIFT + index) & 1
        operands.append(_operand(source(code, 1, literal), neg, absolute))
    return " ".join([f"{name} {', '.join(operands)}", *_modifiers(word, count, mix)])
