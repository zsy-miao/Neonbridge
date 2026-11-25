"""Décodeur des encodages d'instructions RDNA 2 (GPU de la Xbox Series X).

Première étape du recompilateur de shaders (issue #21) : découper un programme en
instructions et reconnaître, pour chacune, sa famille d'encodage, son n° d'opcode et
sa longueur (constante littérale comprise). Les formats viennent de la documentation
publique d'AMD, « RDNA 2 Instruction Set Architecture » (chapitre Microcode Formats).
Les mnémoniques et la sémantique de chaque opcode restent à faire.

    python PrototypeScarlett/scarlett.py --rdna2 <shader.bin>
"""
import struct
from collections import Counter
from dataclasses import dataclass

LITERAL = 255            # opérande « constante littérale » : un mot de plus après l'instruction
DWORD = 4

# (décalage, valeur attendue, famille) pour le premier mot, du plus précis au plus large.
PREFIXES = (
    (23, 0x17D, "SOP1"), (23, 0x17E, "SOPC"), (23, 0x17F, "SOPP"),
    (25, 0x3F, "VOP1"), (25, 0x3E, "VOPC"),
    (26, 0x32, "VINTRP"), (26, 0x33, "VOP3P"), (26, 0x35, "VOP3"), (26, 0x36, "DS"), (26, 0x37, "FLAT"),
    (26, 0x38, "MUBUF"), (26, 0x3A, "MTBUF"), (26, 0x3C, "MIMG"), (26, 0x3D, "SMEM"), (26, 0x3E, "EXP"),
    (28, 0xB, "SOPK"),
    (30, 0x2, "SOP2"),
    (31, 0x0, "VOP2"),
)
# Champ d'opcode de chaque famille : (décalage, masque).
OPCODE = {
    "SOP2": (23, 0x7F), "SOPK": (23, 0x1F), "SOP1": (8, 0xFF), "SOPC": (16, 0x7F), "SOPP": (16, 0x7F),
    "VOP1": (9, 0xFF), "VOPC": (17, 0xFF), "VOP2": (25, 0x3F), "VOP3": (16, 0x3FF), "VOP3P": (16, 0x7F),
    "VINTRP": (16, 0x3), "DS": (18, 0xFF), "FLAT": (18, 0x7F), "MUBUF": (18, 0x7F), "MTBUF": (16, 0x7),
    "MIMG": (18, 0x7F), "SMEM": (18, 0xFF), "EXP": (0, 0),
}
WIDE = frozenset({"VOP3", "VOP3P", "DS", "FLAT", "MUBUF", "MTBUF", "MIMG", "SMEM", "EXP"})
FLAT_SEGMENTS = {0: "FLAT", 1: "SCRATCH", 2: "GLOBAL"}
VOP2_WITH_LITERAL = frozenset({0x20, 0x21, 0x2C, 0x2D, 0x37, 0x38})   # v_madmk/madak_f32, v_fmamk/fmaak_f32 et _f16
SOPK_SETREG_IMM32 = 0x15
VOP_EXTENSIONS = frozenset({0xE9, 0xEA, 0xF9, 0xFA})     # DPP8, DPP8 FI, SDWA, DPP16
OPCODE_HIGH_BIT = {"MIMG": 0, "MUBUF": 25}
MTBUF_OPCODE_BIT3 = 21                   # bit 53 de l'instruction (second mot) : 4e bit de l'opcode MTBUF


@dataclass(frozen=True)
class Instruction:
    offset: int     # en octets depuis le début du programme
    family: str
    opcode: int
    size: int       # en mots de 32 bits


def _family(word):
    for shift, value, family in PREFIXES:
        if word >> shift == value:
            return FLAT_SEGMENTS.get((word >> 14) & 0x3, "FLAT") if family == "FLAT" else family
    return "inconnue"


def _has_literal(family, word, second):
    if family in ("SOP2", "SOPC"):
        return word & 0xFF == LITERAL or (word >> 8) & 0xFF == LITERAL
    if family == "SOP1":
        return word & 0xFF == LITERAL
    if family == "SOPK":
        return (word >> 23) & 0x1F == SOPK_SETREG_IMM32
    if family in ("VOP1", "VOPC"):
        return word & 0x1FF == LITERAL
    if family == "VOP2":
        return word & 0x1FF == LITERAL or (word >> 25) & 0x3F in VOP2_WITH_LITERAL
    if family in ("VOP3", "VOP3P") and second is not None:        # src0, src1, src2 dans le second mot
        return LITERAL in (second & 0x1FF, (second >> 9) & 0x1FF, (second >> 18) & 0x1FF)
    return False


def _has_extension(family, word):
    """SDWA ou DPP : le src0 des VOP1/VOP2/VOPC annonce un mot de contrôle en plus."""
    return family in ("VOP1", "VOP2", "VOPC") and word & 0x1FF in VOP_EXTENSIONS


def _size(family, word, second):
    size = 2 if family in WIDE or family in FLAT_SEGMENTS.values() else 1
    if family == "MIMG":
        size += (word >> 1) & 0x3            # adresses NSA : mots supplémentaires
    if _has_extension(family, word):
        return size + 1
    return size + (1 if _has_literal(family, word, second) else 0)


def _opcode(family, word, second):
    base = "FLAT" if family in FLAT_SEGMENTS.values() else family
    shift, mask = OPCODE.get(base, (0, 0))
    opcode = (word >> shift) & mask
    if base in OPCODE_HIGH_BIT:              # bit OPM : 8e bit de l'opcode
        opcode |= ((word >> OPCODE_HIGH_BIT[base]) & 1) << 7
    if base == "MTBUF" and second is not None:   # MTBUFe_gfx10 de LLVM : Inst{53} = op{3}
        opcode |= ((second >> MTBUF_OPCODE_BIT3) & 1) << 3
    return opcode


def classify(data, offset=0):
    """(famille, opcode, longueur en mots) de l'instruction à `offset`."""
    word = struct.unpack_from("<I", data, offset)[0]
    second = struct.unpack_from("<I", data, offset + DWORD)[0] if len(data) >= offset + 2 * DWORD else None
    family = _family(word)
    return family, _opcode(family, word, second), _size(family, word, second)


# Encodages absents des tests d'encodage gfx10 de LLVM (qui n'ont que la forme VOP3 0x200-0x202) :
# opcodes VINTRP du chapitre « Parameter Interpolation » de la documentation ISA RDNA 2 d'AMD,
# et les deux opcodes MTBUF « xyz » que mtbuf-gfx10.s n'essaie pas (chapitre « MTBUF Instructions »).
ISA_ONLY_MNEMONICS = {("VINTRP", 0): "v_interp_p1_f32", ("VINTRP", 1): "v_interp_p2_f32",
                      ("VINTRP", 2): "v_interp_mov_f32",
                      ("MTBUF", 2): "tbuffer_load_format_xyz", ("MTBUF", 6): "tbuffer_store_format_xyz"}


def mnemonic(family, opcode):
    """Nom de l'instruction (table tirée des tests d'encodage de LLVM, complétée par l'ISA), ou None."""
    from emu.rdna2_ops import MNEMONICS

    found = MNEMONICS.get((family, opcode))     # FLAT, GLOBAL et SCRATCH ont chacun leurs noms
    return found if found is not None else ISA_ONLY_MNEMONICS.get((family, opcode))


def decode(data):
    """Instructions d'un programme RDNA 2 ; une dernière « tronquée » si le binaire est coupé."""
    instructions, offset = [], 0
    while offset + DWORD <= len(data):
        family, opcode, size = classify(data, offset)
        if offset + size * DWORD > len(data):
            instructions.append(Instruction(offset, "tronquée", 0, (len(data) - offset) // DWORD))
            break
        instructions.append(Instruction(offset, family, opcode, size))
        offset += size * DWORD
    return tuple(instructions)


def histogram(instructions):
    """Nombre d'instructions par famille, trié par nom."""
    return dict(sorted(Counter(i.family for i in instructions).items()))


RAW_COLUMN = 27     # trois mots de 32 bits en hexadécimal


def listing(data):
    """Une ligne par instruction : décalage, mots bruts, puis l'instruction au format de LLVM
    (opérandes compris pour les familles couvertes par rdna2_operands), sinon son mnémonique."""
    from emu import rdna2_operands

    lines = []
    for instruction in decode(data):
        raw = struct.unpack_from(f"<{instruction.size}I", data, instruction.offset)
        name = mnemonic(instruction.family, instruction.opcode)
        label = name or f"{instruction.family.lower()}_op_0x{instruction.opcode:X}"
        if instruction.family in rdna2_operands.FAMILIES | set(rdna2_operands.FIXED_FORMAT):
            label = rdna2_operands.text(data, instruction.offset) or label
        words = " ".join(f"{w:08X}" for w in raw)
        lines.append(f"{instruction.offset:06X}  {words:{RAW_COLUMN}} {label}")
    return lines
