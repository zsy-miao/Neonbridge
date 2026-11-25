#!/usr/bin/env python3
"""Check Frosty's Lumen update-grid tile program through the Xbox provider."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import struct
import sys
from tempfile import TemporaryDirectory

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
import pefile
from check_frosty_profile import BUILD
from check_frosty_shaders import GLOBAL_SHA, containers, decoder, decompress, global_archive
from frosty_gpu_guest import run_gpu_guest
from project_tuxbox.inspect import digest

SHADER_SHA = '02b127a5122394daa54fd9200a36419fe34cfe249087133bdd91696aa26b2fbd'
CASES = ('none', 'one', 'last', 'all', 'offset', 'edge')
GUARD = 0xdeadbeef


def view_evidence(mount):
    exe = mount / 'Frosty/Binaries/XSX/Frosty-XSX-Shipping.exe'
    if digest(exe) != BUILD:
        raise ValueError('unexpected Frosty executable')
    checks = {
        0x3a121cf: 'c78424d002000010000000', 0x3a12243: '4889842480020000',
        0x3a12277: 'c78424e00200000c000000', 0x3a122e3: '4889842418010000',
        0x3a12767: 'c784243003000004000000', 0x3a127cf: '4c8bf0',
        0x3a12807: '48c784243004000039000000', 0x3a128f1: '4c8d842440080000',
        0x3a12c39: 'c78424500300001c000000', 0x3a12c4e: '4c89b42458030000',
        0x3a12ca9: 'c78424700300001c000000', 0x3a12cbe: '488b842418010000',
        0x3a12ce5: 'b801000000', 0x3a12cea: '89842414050000',
        0x3a12cf1: '488b842480020000',
    }
    names = [(0x3a121e6, 'Lumen.UpdateBoundsBuffer'), (0x3a1228f, 'Lumen.UpdateIndirectArgBuffer'),
             (0x3a1277e, 'Lumen.UpdateTileBuffer'), (0x3a128f9, 'Lumen.UpdateTileMask')]
    with pefile.PE(str(exe), fast_load=True) as pe:
        for rva, raw in checks.items():
            if pe.get_data(rva, len(bytes.fromhex(raw))) != bytes.fromhex(raw):
                raise ValueError(f'unexpected Lumen resource instruction at {rva:#x}')
        for rva, name in names:
            lea = pe.get_data(rva, 7)
            target = rva + 7 + struct.unpack_from('<i', lea, 3)[0]
            expected = (name + '\0').encode('utf-16le')
            if lea[:2] != bytes.fromhex('488d') or pe.get_data(target, len(expected)) != expected:
                raise ValueError(f'unexpected Lumen name at {rva:#x}')
    return {'executable_sha256': BUILD, 'bounds': 'PF_A32B32G32R32F; 16-byte elements',
            'counter': 'PF_R32_UINT; 12-byte allocation', 'tiles': 'PF_R32_UINT; 4-byte elements',
            'mask': 'PF_R8_UINT; 3D texture',
            'evidence': 'Named allocations and descriptor format instructions are checked in the executable. Bounds, counter and tile views load their corresponding allocation pointers.'}


def words(path, values):
    path.write_bytes(struct.pack(f'<{len(values)}I', *values))


def fbits(value):
    return struct.unpack('<I', struct.pack('<f', value))[0]


def inputs(trial, case):
    count = {'none': 0, 'one': 1, 'last': 65, 'all': 129, 'offset': 65, 'edge': 2}[case]
    step, bias, pad = (((2., -1., 2.), (0.5, 1., 0.), (0.5, 0.5, 0.5)) if case == 'offset'
                       else ((1., 1., 1.), (0., 0., 0.), (0., 0., 0.)))
    cb = [0] * 64
    cb[12], cb[16:19] = count, [2, 2, 2]
    cb[20:23], cb[24:27], cb[28:31] = map(fbits, step), map(fbits, bias), map(fbits, pad)
    words(trial / 'cb.bin', cb)
    bounds = [((100., 100., 100.), (0., 0., 0.)) for _ in range(max(count, 1))]
    if case in ('one', 'last'):
        bounds[-1] = ((1., 0., 1.), (0., 0., 0.))
    if case == 'offset':
        bounds[-1] = ((2.5, 1., 2.), (0., 0., 0.))
    if case == 'edge':
        bounds[-1] = ((0.5, 0.5, 0.5), (0.5, 0.5, 0.5))
    if case == 'all':
        bounds[-1] = ((0.5, 0.5, 0.5), (1., 1., 1.))
    words(trial / 't0.bin', [fbits(v) for center, extent in bounds for vec in (center, extent) for v in (*vec, 0.)])
    words(trial / 'u0.bin', [3, GUARD, GUARD])
    words(trial / 'u1.bin', [GUARD] * 64)
    (trial / 'u2.bin').write_bytes(bytes([0xcc]) * 64)
    active = set()
    for z in range(2):
        for y in range(2):
            for x in range(2):
                if any(all(abs(v * scale + b - u) <= e + padding
                           for v, scale, b, u, e, padding in zip((x, y, z), step, bias, center, extent, pad))
                       for center, extent in bounds[:count]):
                    active.add(x | (y << 8) | (z << 16))
    return active


def verify(trial, active):
    counter = struct.unpack('<3I', (trial / 'out0.bin').read_bytes())
    tiles = struct.unpack('<64I', (trial / 'out1.bin').read_bytes())
    mask = (trial / 'out2.bin').read_bytes()
    if counter != (3 + len(active), GUARD, GUARD):
        raise ValueError(f'counter mismatch: {counter}')
    if set(tiles[3:3 + len(active)]) != active or any(v != GUARD for v in (*tiles[:3], *tiles[3 + len(active):])):
        raise ValueError('tile list or guard mismatch')
    for z in range(4):
        for y in range(4):
            for x in range(4):
                expected = int((x | (y << 8) | (z << 16)) in active) if x < 2 and y < 2 and z < 2 else 0xcc
                if mask[z * 1024 + y * 256 + x] != expected:
                    raise ValueError(f'mask mismatch at {(x, y, z)}')
    return {'active_tiles': len(active), 'mask_texels_checked': 64, 'tile_words_checked': 64, 'counter_words_checked': 3}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('mount', 'xshader', 'provider', 'proton', 'prefix', 'compiler-dir', 'work', 'output'):
        parser.add_argument('--' + name, type=Path, required=True)
    args = parser.parse_args()
    work, proton = args.work.resolve(), args.proton.resolve()
    work.mkdir(parents=True, exist_ok=True)
    view = view_evidence(args.mount)
    archive, entries = global_archive(args.mount)
    ((_, native, _),) = containers(decompress(archive, entries[2136], decoder()))
    if hashlib.sha256(native).hexdigest() != SHADER_SHA:
        raise ValueError('unexpected global 2136 shader')
    os.environ['TUXBOX_XSHADER'] = str(args.xshader.resolve())
    cases, adapters, guest_hash = {}, set(), None
    for case in CASES:
        with TemporaryDirectory(dir=work) as temp:
            trial = Path(temp)
            (trial / 'retail.dxbc').write_bytes(native)
            active = inputs(trial, case)
            exe, status, actual = run_gpu_guest(ROOT / 'tools/reference/lumen-update-tiles/guest.cpp', 'guest.exe',
                                                trial, work, proton, args.prefix, args.provider, args.compiler_dir)
            if status or actual.get('status') != 'ok':
                raise RuntimeError(f'{case}: {status}, {actual}. See {work / "gpu.log"}')
            guest_hash = digest(exe)
            adapters.update(line.split('Found device: ', 1)[1] for line in (work / 'gpu.log').read_text().splitlines()
                            if 'Found device: ' in line)
            cases[case] = verify(trial, active)
    result = {'scope': 'Lumen global 2136 through the provider and runtime shader compiler. Finite bounds and guard checks; not an end-to-end lighting test.',
              'archive_sha256': GLOBAL_SHA, 'shader_sha256': SHADER_SHA, 'view': view, 'adapters': sorted(adapters),
              'proton': (proton / 'version').read_text().strip(), 'xshader_sha256': digest(args.xshader),
              'provider_sha256': digest(args.provider), 'guest_sha256': guest_hash, 'cases': cases}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2) + '\n')


if __name__ == '__main__':
    main()
