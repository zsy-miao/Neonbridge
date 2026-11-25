"""Couverture du désassembleur : chaque encodage fabriqué est relu tel quel et écrit avec ses opérandes."""
from emu import rdna2, rdna2_coverage, rdna2_operands
from emu.rdna2_ops import MNEMONICS


def test_encodings_round_trip_through_the_decoder():
    for family, opcode in MNEMONICS:
        found = rdna2.classify(rdna2_coverage.encode(family, opcode))[:2]
        assert found == (family, opcode), (family, opcode)


def test_every_table_encoding_is_disassembled():
    done, total = rdna2_coverage.coverage()
    assert total == len(MNEMONICS) and done == total


def test_an_encoding_without_operand_text_is_not_counted(monkeypatch):
    monkeypatch.setattr(rdna2_operands, "text", lambda data, offset=0: None)
    assert not rdna2_coverage.disassembled("VOP1", 1)
