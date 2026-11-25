"""Couverture du désassembleur RDNA 2 : encodages relus et écrits avec leurs opérandes (syntaxe LLVM).

Pour chaque (famille, opcode) de la table, on fabrique un encodage minimal (préfixe de la famille,
opcode, autres champs à zéro), on vérifie que le décodeur le reconnaît et que
emu/rdna2_operands.py en écrit le texte complet. Rien n'est déclaré à la main.
"""
from emu import rdna2, rdna2_operands

PADDING_WORDS = 4                      # littéral, NSA ou mot d'extension éventuels : zéros
SEGMENTS = {"FLAT": 0, "SCRATCH": 1, "GLOBAL": 2}
SEGMENT_SHIFT = 14


def _prefix(family):
    base = "FLAT" if family in SEGMENTS else family
    for shift, value, name in rdna2.PREFIXES:
        if name == base:
            return shift, value, base
    raise KeyError(family)


def encode(family, opcode):
    """Octets d'un encodage minimal de (famille, opcode), suivis de mots nuls."""
    shift, value, base = _prefix(family)
    word = value << shift
    op_shift, mask = rdna2.OPCODE.get(base, (0, 0))
    word |= (opcode & mask) << op_shift
    if base in rdna2.OPCODE_HIGH_BIT:
        word |= ((opcode >> 7) & 1) << rdna2.OPCODE_HIGH_BIT[base]
    if family in SEGMENTS:
        word |= SEGMENTS[family] << SEGMENT_SHIFT
    second = ((opcode >> 3) & 1) << rdna2.MTBUF_OPCODE_BIT3 if base == "MTBUF" else 0
    return word.to_bytes(4, "little") + second.to_bytes(4, "little") + bytes(4 * PADDING_WORDS)


def disassembled(family, opcode):
    """Vrai si l'encodage est reconnu tel quel et écrit avec ses opérandes."""
    data = encode(family, opcode)
    found_family, found_opcode, _ = rdna2.classify(data)
    if (found_family, found_opcode) != (family, opcode):
        return False
    return rdna2_operands.text(data) is not None


def by_family():
    """{famille: (encodages désassemblés, encodages connus)}."""
    from emu.rdna2_ops import MNEMONICS

    counts = {}
    for family, opcode in MNEMONICS:
        done, total = counts.get(family, (0, 0))
        counts[family] = (done + disassembled(family, opcode), total + 1)
    return counts


def coverage():
    """(encodages désassemblés, encodages connus)."""
    counts = by_family().values()
    return sum(d for d, _ in counts), sum(t for _, t in counts)
