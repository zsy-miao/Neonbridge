"""Seed ProjectScarlett's shader request archive from the title's shader pack.

ProjectScarlett normally records each shader the game requests during a run,
and the launcher prepares those requests before the next launch. A shader
first requested in a new scene therefore still compiles while the game waits.

``shaders.rpf`` stores each shader as a plain record after an encrypted table
of contents. The records can be found without the table, so nothing here
decrypts or bypasses anything. Each record is laid out as:

    u8 n | name (n - 1 bytes) | 0x00 | u8 flag | u32 csize | u32 ? | u32 csize | u32 usize | deflate[csize]

The payload inflates to a DXBC container. XCL documents the same layout in
its shader-pack reader (``crates/xcl/src/gpu/pack.rs``); this is an
independent implementation for TuxBox.

Only standalone ``.dxbc`` requests can be seeded. Linked vertex/pixel pairs,
stencil variants and programs that receive their root signature at pipeline
creation are still discovered by playing.
"""
import hashlib
import mmap
import os
from pathlib import Path
import re
import zlib

from .runtime_shaders import MAX_SHADER_BYTES

# n counts the name and its terminator. A name starts with a letter or an
# underscore; an effect-qualified name contains one colon.
_RECORD = re.compile(rb"[A-Za-z_][A-Za-z0-9_:]{2,253}\x00", re.DOTALL)
_HEADER = 17  # flag, csize, unknown, csize, usize


def _u32(data, at):
    return int.from_bytes(data[at:at + 4], "little")


def records(data):
    """Yield ``(name, offset, payload)`` for each shader record in ``data``.

    ``payload`` is the inflated DXBC container. Scanning resumes after an
    accepted payload, so a suffix of a record's name that happens to look like
    a record header is never reported as a second record.
    """
    pos = 1
    size = len(data)
    while True:
        match = _RECORD.search(data, pos)
        if match is None:
            return
        start, terminator = match.start(), match.end() - 1
        # The name may begin later in the match; the length byte decides.
        accepted = None
        for begin in range(start, terminator - 2):
            length = terminator - begin + 1
            if begin == 0 or data[begin - 1] != length:
                continue
            if not (data[begin:begin + 1].isalpha() or data[begin] == 0x5F):
                continue
            header = terminator + 1
            if header + _HEADER > size:
                break
            csize, csize2, usize = _u32(data, header + 1), _u32(data, header + 9), _u32(data, header + 13)
            payload_at = header + _HEADER
            if (csize == 0 or csize != csize2 or not 32 <= usize <= MAX_SHADER_BYTES
                    or payload_at + csize > size):
                continue
            try:
                inflater = zlib.decompressobj(-zlib.MAX_WBITS)
                payload = inflater.decompress(data[payload_at:payload_at + csize], MAX_SHADER_BYTES + 1)
            except zlib.error:
                continue
            if len(payload) != usize or payload[:4] != b"DXBC":
                continue
            accepted = (bytes(data[begin:terminator]).decode("ascii"), begin - 1, payload, payload_at + csize)
            break
        if accepted is None:
            pos = start + 1
            continue
        name, offset, payload, end = accepted
        yield name, offset, payload
        pos = end + 1


def seed_requests(pack, requests, report=None):
    """Add each shader in ``pack`` to ``requests`` as ``<sha256>.dxbc``.

    Existing requests are kept. Returns ``(found, added)``.
    """
    requests = Path(requests)
    requests.mkdir(parents=True, exist_ok=True)
    found = added = 0
    with open(pack, "rb") as stream:
        if os.fstat(stream.fileno()).st_size == 0:
            return 0, 0
        with mmap.mmap(stream.fileno(), 0, access=mmap.ACCESS_READ) as data:
            seen = set()
            for _, _, payload in records(data):
                key = hashlib.sha256(payload).hexdigest()
                if key in seen:
                    continue
                seen.add(key)
                found += 1
                dest = requests / (key + ".dxbc")
                if dest.exists():
                    continue
                staged = dest.with_name(dest.name + ".seed")
                staged.write_bytes(payload)
                try:
                    os.link(staged, dest)
                    added += 1
                except FileExistsError:
                    pass
                finally:
                    staged.unlink()
                if report and found % 5000 == 0:
                    report(f"Shader pack: {found} shaders found, {added} new")
    return found, added
