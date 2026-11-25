"""Read the zlib-packed Unreal v1 symbol-range and name tables found in Xbox packages."""
from bisect import bisect_left
import io
import os
import struct
import zlib


def exact(stream, n):
    data = stream.read(n)
    if len(data) != n:
        raise ValueError("Truncated Unreal symbol database")
    return data


def symbol_header(stream, unpacked):
    version, length = struct.unpack("<II", exact(stream, 8))
    if version != 1 or not 1 < length <= 2048:
        raise ValueError("Unrecognized Unreal symbol header")
    name = exact(stream, length)
    if name[-1] or not name[:-1].isascii() or not name[:-1].decode("ascii").isprintable():
        raise ValueError("Invalid Unreal symbol module name")
    _, count = struct.unpack("<QI", exact(stream, 12))
    # Each range has three DWORDs and a corresponding length-prefixed, terminated name.
    if 24 + length + count * 17 > unpacked:
        raise ValueError("Symbol tables exceed the declared uncompressed size")
    return {"kind": "Unreal symbol database", "module": name[:-1].decode("ascii"),
            "symbol_count": count, "unpacked_bytes": unpacked}


class _ZlibReader(io.RawIOBase):
    """Inflate into the bounded buffers requested by BufferedReader."""

    def __init__(self, source, size):
        self.source = source
        self.remaining = size
        self.inflate = zlib.decompressobj()

    def readable(self):
        return True

    def readinto(self, target):
        if not target:
            return 0
        while not self.inflate.eof:
            chunk = self.inflate.unconsumed_tail or self.source.read(65536)
            try:
                output = self.inflate.decompress(chunk, len(target))
            except zlib.error as exc:
                raise ValueError(f"Invalid symbol compression: {exc}") from exc
            self.remaining -= len(output)
            if self.remaining < 0:
                raise ValueError("Symbol stream exceeds its declared size")
            if output:
                target[:len(output)] = output
                return len(output)
            if not chunk and not self.inflate.eof:
                raise ValueError("Truncated compressed symbol stream")
        if self.remaining or self.inflate.unused_data or self.source.read(1):
            raise ValueError("Symbol stream length does not match its header")
        return 0


def lookup_symbols(path, sites):
    """Resolve PE-relative addresses, retaining only the requested symbols in memory."""
    sites = list(dict.fromkeys(sites))
    requested = sorted(sites)
    if any(not 0 <= addr <= 0xffffffff for addr in requested):
        raise ValueError("RVA must be between 0 and 0xffffffff")
    with path.open("rb") as packed:
        raw_size, packed_size = struct.unpack("<II", exact(packed, 8))
        if packed_size + 8 != os.fstat(packed.fileno()).st_size:
            raise ValueError("Compressed symbol size does not match the file")
        if max(raw_size, packed_size) > 512 << 20:
            raise ValueError("Symbol lookup exceeds the 512 MiB input or decompression limit")
        with io.BufferedReader(_ZlibReader(packed, raw_size), buffer_size=65536) as stream:
            result = symbol_header(stream, raw_size)
            count = result["symbol_count"]
            matches = {}
            for first in range(0, count, 4096):
                records = exact(stream, min(count - first, 4096) * 12)
                for i, (start, length, _) in enumerate(struct.iter_unpack("<III", records), first):
                    end = start + length
                    if end > 1 << 32:
                        raise ValueError("Invalid symbol RVA range")
                    at = bisect_left(requested, start)
                    while at < len(requested) and requested[at] < end:
                        addr = requested[at]
                        # Identical code folding can put several names at one address.
                        matches[addr] = None if addr in matches else (i, start)
                        at += 1
            if struct.unpack("<I", exact(stream, 4))[0] != count:
                raise ValueError("Symbol and name counts differ")
            names = {hit[0]: None for hit in matches.values() if hit is not None}
            for i in range(count):
                length, = struct.unpack("<I", exact(stream, 4))
                if not 0 < length <= 1 << 20:
                    raise ValueError("Invalid symbol name length")
                encoded = exact(stream, length)
                if encoded[-1] or b"\0" in encoded[:-1]:
                    raise ValueError("Invalid symbol name terminator")
                name = encoded[:-1].decode("utf-8")
                if i in names:
                    names[i] = name
            if stream.read(1):
                raise ValueError("Extra data after the symbol table")

    result["lookups"] = []
    result["size_bytes"] = packed_size + 8
    for addr in sites:
        row = {"rva": hex(addr)}
        if addr not in matches:
            row["status"] = "unmapped"
        elif matches[addr] is None:
            row["status"] = "ambiguous"
        else:
            index, start = matches[addr]
            row.update(status="matched", function=names[index], function_rva=hex(start), offset=hex(addr - start))
        result["lookups"].append(row)
    return result
