"""Passerelle Rust vers le décodeur RDNA 2 de TomBox (rust/anyxboxx-rdna2, sous-module third_party/tombox)."""
import subprocess
from pathlib import Path

import pytest

BINARY = Path(__file__).resolve().parents[1] / "rust" / "target" / "release" / "anyxboxx_rdna2.exe"
pytestmark = pytest.mark.skipif(not BINARY.is_file(), reason="passerelle Rust non compilée (cargo build --release)")


def decode(*lines):
    result = subprocess.run([str(BINARY)], input="\n".join(lines) + "\n", capture_output=True, text=True,
                            timeout=60, check=True)
    return result.stdout.splitlines()


def test_tombox_decodes_lengths_and_names():
    assert decode("00 00 81 bf", "ff 02 00 80 73 72 71 3f") == ["1 Sopp s_endpgm", "2 Sop2 s_add_u32"]


def test_unreadable_lines_are_reported():
    assert decode("zz") == ["0 erreur -"]
