#!/usr/bin/env python3
"""Count call sites per import of a PE executable (offline; idea from leonida
aa7223ec, re-derived). For each IAT slot of the delay-load imports (and, with
--all, the ordinary imports) it counts the RIP-relative references in
executable sections: call [rip+x] (FF 15), jmp [rip+x] (FF 25 / 48 FF 25) and
mov reg,[rip+x] (48/4C 8B modrm 05..3D), so the imports the title really uses
can be ranked against our stubs.

usage: python import_call_sites.py GAME.exe [--all] [--top N] [--csv OUT]
"""
import struct
import sys
from collections import Counter


def u16(b, o): return struct.unpack_from("<H", b, o)[0]
def u32(b, o): return struct.unpack_from("<I", b, o)[0]
def u64(b, o): return struct.unpack_from("<Q", b, o)[0]


class Pe:
    def __init__(self, data):
        self.data = data
        nt = u32(data, 0x3C)
        if data[nt:nt + 4] != b"PE\0\0":
            raise SystemExit("not a PE file")
        fh = nt + 4
        sections = u16(data, fh + 2)
        opt = fh + 20
        if u16(data, opt) != 0x20B:
            raise SystemExit("not a PE32+ image")
        self.base = u64(data, opt + 24)
        self.dirs = opt + 112
        self.sections = []
        at = opt + u16(data, fh + 16)
        for i in range(sections):
            s = at + 40 * i
            name = data[s:s + 8].rstrip(b"\0").decode(errors="replace")
            vsize, va, rsize, raw, flags = u32(data, s + 8), u32(data, s + 12), u32(data, s + 16), u32(data, s + 20), u32(data, s + 36)
            self.sections.append((name, va, max(vsize, rsize), raw, rsize, flags))

    def directory(self, index):
        return u32(self.data, self.dirs + 8 * index), u32(self.data, self.dirs + 8 * index + 4)

    def off(self, rva):
        for _, va, size, raw, rsize, _ in self.sections:
            if va <= rva < va + size and rva - va < rsize:
                return raw + rva - va
        return None

    def cstr(self, rva):
        o = self.off(rva)
        if o is None:
            return "?"
        end = self.data.index(b"\0", o)
        return self.data[o:end].decode(errors="replace")

    def thunks(self, names_rva, iat_rva):
        """(iat slot rva, import name) for one DLL's name/IAT arrays."""
        i = 0
        while True:
            o = self.off(names_rva + 8 * i)
            if o is None:
                return
            entry = u64(self.data, o)
            if entry == 0:
                return
            name = f"#{entry & 0xFFFF}" if entry >> 63 else self.cstr((entry & 0x7FFFFFFF) + 2)
            yield iat_rva + 8 * i, name
            i += 1

    def delay_imports(self):
        rva, size = self.directory(13)
        o = self.off(rva) if rva else None
        while o is not None:
            attrs, dll, _, iat, names = (u32(self.data, o + k) for k in (0, 4, 8, 12, 16))
            if dll == 0:
                break
            if not attrs & 1:  # old VA-based descriptors
                dll, iat, names = (x - self.base for x in (dll, iat, names))
            lib = self.cstr(dll)
            for slot, name in self.thunks(names, iat):
                yield slot, f"{lib}!{name}", "delay"
            o += 32

    def imports(self):
        rva, _ = self.directory(1)
        o = self.off(rva) if rva else None
        while o is not None:
            names, _, _, dll, iat = (u32(self.data, o + k) for k in (0, 4, 8, 12, 16))
            if dll == 0:
                break
            lib = self.cstr(dll)
            for slot, name in self.thunks(names or iat, iat):
                yield slot, f"{lib}!{name}", "import"
            o += 20


def references(pe, slots):
    """Count RIP-relative references to each slot rva in executable sections."""
    counts = Counter()
    for name, va, _, raw, rsize, flags in pe.sections:
        if not flags & 0x20000000:  # IMAGE_SCN_MEM_EXECUTE
            continue
        code = pe.data[raw:raw + rsize]
        for opcode, disp_at in ((b"\xFF\x15", 2), (b"\xFF\x25", 2)):
            start = 0
            while (i := code.find(opcode, start)) != -1:
                start = i + 1
                if i + 6 > len(code):
                    break
                target = va + i + 6 + struct.unpack_from("<i", code, i + 2)[0]
                if target in slots:
                    counts[target] += 1
        # mov r64, [rip+disp32]: REX.W (48/4C) 8B modrm with mod=00 rm=101.
        for rex in (b"\x48\x8B", b"\x4C\x8B"):
            start = 0
            while (i := code.find(rex, start)) != -1:
                start = i + 1
                if i + 7 > len(code) or code[i + 2] & 0xC7 != 0x05:
                    continue
                target = va + i + 7 + struct.unpack_from("<i", code, i + 3)[0]
                if target in slots:
                    counts[target] += 1
    return counts


def main(argv):
    if not argv or argv[0] in ("-h", "--help"):
        print(__doc__)
        return 0
    path = argv[0]
    everything = "--all" in argv
    top = int(argv[argv.index("--top") + 1]) if "--top" in argv else 0
    csv = argv[argv.index("--csv") + 1] if "--csv" in argv else None
    pe = Pe(open(path, "rb").read())
    entries = list(pe.delay_imports())
    if everything:
        entries += list(pe.imports())
    slots = {slot: (name, kind) for slot, name, kind in entries}
    counts = references(pe, slots)
    rows = sorted(((counts[s], n, k) for s, (n, k) in slots.items()), key=lambda r: (-r[0], r[1]))
    if top:
        rows = rows[:top]
    used = sum(1 for c, _, _ in rows if c)
    print(f"{len(slots)} import slots ({'delay + ordinary' if everything else 'delay-load only'}), {used} referenced")
    for count, name, kind in rows:
        print(f"{count:6} {kind:6} {name}")
    if csv:
        with open(csv, "w") as f:
            f.write("calls,kind,import\n")
            for count, name, kind in rows:
                f.write(f"{count},{kind},{name}\n")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
