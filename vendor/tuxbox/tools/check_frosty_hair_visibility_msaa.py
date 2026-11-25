#!/usr/bin/env python3
"""Dispatch twelve packaged hair compute shaders against sample-frequency raster input."""
import argparse
import json
from pathlib import Path
import shutil
import struct
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from build import Toolchain
from check_frosty_shaders import containers, decoder, decompress, global_archive
from project_tuxbox.host import Host
from project_tuxbox.inspect import digest

# Entry, native multisample count, group height, indirect tile list.
ENTRIES = [(985, 2, 4, False), (999, 2, 8, False), (991, 2, 4, True), (1002, 2, 8, True),
           (1001, 4, 4, False), (1003, 4, 8, False), (987, 4, 4, True), (983, 4, 8, True),
           (980, 8, 4, False), (997, 8, 8, False), (988, 8, 4, True), (989, 8, 8, True)]
GUARD = 0xd15ea5ed
SLOTS = 520


def oracle(pixel, samples, sort):
    nodes = {}
    mode = pixel % 5
    for sample in range(samples):
        if mode == 0 or (mode == 3 and not sample % 2) or (mode == 4 and sample % 2):
            continue
        key = 1 + pixel * 16 + (0 if mode == 2 and sample == 1 else sample)
        depth = (8 + sample) / 32
        previous_depth, occurrences = nodes.get(key, (0, 0))
        nodes[key] = (max(previous_depth, depth), occurrences + 1)
    accepted = sum(n for _, n in nodes.values())
    ordered = sorted(nodes, key=lambda k: -nodes[k][0]) if sort else nodes
    records = []
    for key in ordered:
        depth, count = nodes[key]
        packed_depth = int(depth * 16777215) << 8
        packed_weight = min(255, (256 * count) // accepted)
        records.append((packed_depth | packed_weight, key))
    return records


def verify(work, entry, samples, height, sort):
    def read(slot):
        data = (work / f'{entry}-{sort}-u{slot}.bin').read_bytes()
        return struct.unpack(f'<{len(data) // 4}I', data)
    counter, index, records, coords, coverage = map(read, range(5))
    occupied = set()
    for y in range(8):
        for x in range(8):
            pixel = 8 * y + x
            value, weight = index[64 * y + x], coverage[64 * y + x]
            if y >= height:
                if (value, weight) != (GUARD, GUARD):
                    raise ValueError(f'{entry}/{sort}: modified untouched pixel {pixel}')
                continue
            expected = oracle(pixel, samples, sort)
            base, count = value & 0x3ffffff, value >> 26
            if count != len(expected) or weight != (0x3f400000 if count else 0):
                raise ValueError(f'{entry}/{sort}: pixel {pixel}, count {count}/{len(expected)}, weight {weight:#x}')
            for j, pair in enumerate(expected):
                at = base + j
                pair_slice = slice(2 * at, 2 * at + 2)
                actual = records[pair_slice]
                if at in occupied or actual != pair or coords[pair_slice] != (x, y):
                    raise ValueError(f'{entry}/{sort}: pixel {pixel} node {j}, position {at}, actual {actual}, '
                                     f'expected {pair}, coords {coords[pair_slice]}')
                occupied.add(at)
    if counter[0] != 5 + len(occupied) or occupied != set(range(5, counter[0])):
        raise ValueError(f'{entry}/{sort}: allocation counter {counter[0]}, occupied {len(occupied)}')
    for at in set(range(SLOTS)) - occupied:
        pair_slice = slice(2 * at, 2 * at + 2)
        if records[pair_slice] != (GUARD, GUARD) or coords[pair_slice] != (GUARD, GUARD):
            raise ValueError(f'{entry}/{sort}: changed guard {at}')
    return len(occupied)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    inputs = parser.add_mutually_exclusive_group(required=True)
    inputs.add_argument('--mount', type=Path, help='Mounted title containing the global shader archive')
    inputs.add_argument('--native-dir', type=Path, help='Previously extracted numbered DXBC files')
    for name in ('compiler', 'dxc', 'proton', 'prefix', 'work', 'output'):
        parser.add_argument('--' + name, type=Path, required=True)
    args = parser.parse_args()
    work = args.work.resolve()
    work.mkdir(parents=True, exist_ok=True)
    source = ROOT / 'tools/reference/hair-visibility-msaa'
    dxc = args.dxc.resolve()
    toolchain = Toolchain()
    obj = toolchain.compile(source / 'guest.cpp', work, f'/I{ROOT/"runtime/d3d12"}')
    exe = work / 'probe.exe'
    toolchain.link(exe, obj, 'd3d12.lib', 'dxgi.lib', 'kernel32.lib')
    for entry in ['vs', 'ps']:
        subprocess.run([dxc, '-T', entry+'_6_0', '-E', entry, '-Fo', work/(entry+'.cso'), source / 'samples.hlsl'], check=True)
    cases = []
    if args.mount:
        payload, entries = global_archive(args.mount)
        decode = decoder()
    for entry, samples, _, indirect in ENTRIES:
        native = work / f'{entry}-native.cso'
        if args.mount:
            ((_, code, _),) = containers(decompress(payload, entries[entry], decode))
            native.write_bytes(code)
        else:
            shutil.copyfile(args.native_dir / f'{entry}.dxbc', native)
        hlsl, profile = work / f'{entry}.hlsl', work / f'{entry}.profile'
        subprocess.run([args.compiler, native, '--hlsl-out', hlsl, '--profile-out', profile], check=True, stdout=subprocess.DEVNULL)
        subprocess.run([dxc, '-T', profile.read_text().strip(), '-E', 'main', '-Fo', work / f'{entry}.cso', hlsl], check=True)
        cases.append(f'{entry} {samples} {int(indirect)}')
    (work / 'cases.txt').write_text('\n'.join(cases)+'\n')
    run = Host(None, args.proton.resolve(), args.prefix.resolve()).run(exe, [], capture=True, timeout=120)
    receipt = (work / 'result.txt').read_text()
    if run.returncode or len(receipt.splitlines()) != 1 + len(cases):
        raise RuntimeError(f'GPU run {run.returncode}: {receipt}')
    result = {}
    for entry, samples, height, _ in ENTRIES:
        result[entry] = {'samples': samples, 'group_height': height, 'guest_sha256': digest(work / f'{entry}-native.cso'),
                         'dxil_sha256': digest(work / f'{entry}.cso'),
                         'verified_records': [verify(work, entry, samples, height, order) for order in range(2)]}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(dict(adapter=receipt.splitlines()[0],scope='Multisample texture IDs and depths generated by sample-frequency pixel shading. Five UAVs read after each packaged compute dispatch; CPU oracle checks empty, separate, merged, missing and depth-rejected samples, incoming and descending depth order, buffer and pixel guards.',entries=result),indent=2)+'\n')


if __name__ == '__main__':
    main()
