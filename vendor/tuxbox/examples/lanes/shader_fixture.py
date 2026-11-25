"""The synthetic Xbox compute shader the Lanes self-test loads.

It is a small hand-written console program, not taken from any game: one
`v_mov_b32` and `s_endpgm`, with an empty root signature embedded.
"""
import hashlib
from pathlib import Path
import struct
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
from tools.reference.scarlett_worker_fixture import container, pack  # noqa: E402

CODE = (0x7E0002FF, 0x12345678, 0xBF810000)
# Root signature 1.0 with no parameters, samplers or flags.
EMPTY_ROOT = pack([1, 0, 24, 0, 24, 0])


def shader():
    base = container(list(CODE))
    count = struct.unpack_from("<I", base, 28)[0]
    offsets = struct.unpack_from(f"<{count}I", base, 32)
    chunks = [base[start:start + 8 + struct.unpack_from("<I", base, start + 4)[0]] for start in offsets]
    chunks.append(b"RTS0" + pack([len(EMPTY_ROOT)]) + EMPTY_ROOT)
    start = 32 + 4 * len(chunks)
    table = []
    for chunk in chunks:
        table.append(start)
        start += len(chunk)
    return b"DXBC" + bytes(16) + pack([1, start, len(chunks), *table]) + b"".join(chunks)


def header(data):
    values = ",".join(str(byte) for byte in data)
    return ("// Generated from shader_fixture.py.\n"
            f"constexpr unsigned char xbox_shader[] = {{{values}}};\n"
            "// The name its translation is cached under.\n"
            f"constexpr wchar_t xbox_shader_digest[] = L\"{hashlib.sha256(data).hexdigest()}\";\n")


if __name__ == "__main__":
    Path(sys.argv[1]).write_text(header(shader()))
