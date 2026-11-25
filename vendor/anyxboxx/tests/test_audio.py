"""Son console (rust/anyxboxx-audio) : un fichier RIFF XMA2 passe par le décodeur de TomBox (portage
Rust de FFmpeg) et ressort en WAV PCM 16 bits relisible par le module wave de Python."""
import struct
import subprocess
import wave
from pathlib import Path

import pytest

BINARY = Path(__file__).resolve().parents[1] / "rust" / "target" / "release" / "anyxboxx_xma.exe"
pytestmark = pytest.mark.skipif(not BINARY.is_file(), reason="crate Rust non compilée (cargo build --release)")
XMA2 = 0x0166


def xma2_file(channels, rate, packets):
    fmt = struct.pack("<HHIIHHHH", XMA2, channels, rate, 0, 0, 0, 34, 1) + bytes(32)
    data = bytes([0xA5]) * 2048 * packets
    body = b"WAVE" + b"fmt " + struct.pack("<I", len(fmt)) + fmt + b"data" + struct.pack("<I", len(data)) + data
    return b"RIFF" + struct.pack("<I", len(body)) + body


def test_xma2_file_becomes_a_readable_wav(tmp_path):
    source, output = tmp_path / "son.xma", tmp_path / "son.wav"
    source.write_bytes(xma2_file(2, 48000, 2))
    result = subprocess.run([str(BINARY), str(source), str(output)], capture_output=True, text=True, timeout=60)
    assert result.returncode == 0, result.stderr
    with wave.open(str(output)) as audio:
        assert (audio.getnchannels(), audio.getframerate(), audio.getsampwidth()) == (2, 48000, 2)


def test_non_xma_input_is_refused(tmp_path):
    source = tmp_path / "pcm.wav"
    source.write_bytes(xma2_file(2, 48000, 1).replace(struct.pack("<H", XMA2), struct.pack("<H", 1), 1))
    result = subprocess.run([str(BINARY), str(source), str(tmp_path / "x.wav")], capture_output=True, text=True,
                            timeout=60)
    assert result.returncode == 1 and "XMA2" in result.stderr
