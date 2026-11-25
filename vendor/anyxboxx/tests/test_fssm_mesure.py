"""tools/fssm_mesure.py sur des FSSM fabriqués (aucune donnée de jeu)."""
import struct
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tools"))
import fssm_mesure  # noqa: E402
from test_rdna2_spirv import CODE_WITH_DATA, COMPUTE_SHADER, DATA_WORD  # noqa: E402

CODE_OFFSET = 0x1D8                       # le code commence à CODE_OFFSET + 0x28 = 0x200


def fssm_blob(code):
    header = bytearray(CODE_OFFSET + 0x28)
    struct.pack_into("<I", header, 0xC0, CODE_OFFSET)
    struct.pack_into("<4I", header, 0x138, 0, 64, 1, 1)           # rsrc2, taille du groupe
    return bytes(header) + code


def test_code_range_follows_the_header_and_the_last_endpgm():
    code = COMPUTE_SHADER[:4] + DATA_WORD + COMPUTE_SHADER[4:]
    assert fssm_mesure.code_range(fssm_blob(code)) == (0x200, 0x200 + len(code))
    assert fssm_mesure.code_range(b"FSSM") is None


def test_measure_counts_without_writing_any_shader(tmp_path):
    (tmp_path / "fssm").mkdir()
    (tmp_path / "fssm" / "deux_morceaux.fssm").write_bytes(fssm_blob(CODE_WITH_DATA))
    (tmp_path / "fssm" / "vide.fssm").write_bytes(b"FSSM" + bytes(8))
    counts = fssm_mesure.measure(tmp_path)
    assert counts["traduction"] == {"traduit": 1, "code introuvable": 1}
    assert counts["instructions"] == 4 and counts["sans texte"] == 1        # le mot de données
    assert sorted(p.name for p in tmp_path.rglob("*")) == ["deux_morceaux.fssm", "fssm", "vide.fssm"]
