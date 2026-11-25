"""Textures console en Rust (rust/anyxboxx-texture, portage du décodeur BC7 de Xbox2Windows, MIT) :
la ligne de commande écrit un PNG que Python relit indépendamment (zlib, CRC)."""
import struct
import subprocess
import zlib
from pathlib import Path

import pytest

BINARY = Path(__file__).resolve().parents[1] / "rust" / "target" / "release" / "anyxboxx_texture.exe"
pytestmark = pytest.mark.skipif(not BINARY.is_file(), reason="crate Rust non compilée (cargo build --release)")


def hand_block():
    """Mode 1, partition 13 (calcul à la main dans les tests de Xbox2Windows)."""
    fields = ((1, 1), (13, 2), (63, 8), (63, 26), (1, 80), (7, 105))
    return sum(value << offset for value, offset in fields).to_bytes(16, "little")


def read_png(path):
    data = path.read_bytes()
    assert data[:8] == b"\x89PNG\r\n\x1a\n"
    position, chunks = 8, {}
    while position < len(data):
        length, kind = struct.unpack_from(">I4s", data, position)
        body = data[position + 8:position + 8 + length]
        crc = struct.unpack_from(">I", data, position + 8 + length)[0]
        assert crc == zlib.crc32(kind + body), kind
        chunks[kind] = chunks.get(kind, b"") + body
        position += 12 + length
    width, height = struct.unpack_from(">II", chunks[b"IHDR"])
    raw = zlib.decompress(chunks[b"IDAT"])
    stride = width * 4 + 1
    assert all(raw[row * stride] == 0 for row in range(height))
    return width, height, b"".join(raw[row * stride + 1:(row + 1) * stride] for row in range(height))


def test_cli_decodes_a_block_into_a_valid_png(tmp_path):
    source, image = tmp_path / "bloc.bin", tmp_path / "bloc.png"
    source.write_bytes(hand_block())
    result = subprocess.run([str(BINARY), "bc7", str(source), "4", "4", str(image)], capture_output=True,
                            text=True, timeout=60)
    assert result.returncode == 0, result.stderr
    width, height, pixels = read_png(image)
    expected = bytes([255, 2, 2, 255]) * 8 + bytes([253, 0, 0, 255]) + bytes([0, 0, 0, 255]) * 7
    assert (width, height, pixels) == (4, 4, expected)


def test_cli_rejects_a_truncated_surface(tmp_path):
    source = tmp_path / "court.bin"
    source.write_bytes(bytes(15))
    result = subprocess.run([str(BINARY), "bc7", str(source), "4", "4", str(tmp_path / "x.png")],
                            capture_output=True, text=True, timeout=60)
    assert result.returncode == 1 and "attendus" in result.stderr
