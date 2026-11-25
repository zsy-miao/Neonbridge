#!/usr/bin/env python3
"""Validate a local retail sphere's raw BVH bounds and malformed copies."""
import argparse
import hashlib
import json
from pathlib import Path
import struct
import subprocess
from tempfile import TemporaryDirectory

from check_frosty_serialized_blas import MESHES, read_asset

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('mount', type=Path)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    mesh = MESHES['sphere']
    asset = read_asset(args.mount, mesh)
    original = asset[mesh.blas_offset:mesh.blas_offset + mesh.blas_size]
    digest = hashlib.sha256(original).hexdigest()
    if digest != '38bd71be9dd00e74952a9612f87f7df37a1617ca9f9847bb531b40bc5a4ad6af':
        raise ValueError('unexpected serialized sphere')
    body = memoryview(original)[64:]
    root = struct.unpack_from('<I', body, 48)[0]
    if root & 7 != 4:
        raise ValueError('control expects the sphere BOX16 root')
    root_offset = 64 + (root >> 3) * 64
    children = struct.unpack_from('<4I', original, root_offset)
    child = next(i for i, value in enumerate(children) if value != 0xffffffff)
    bounds = root_offset + 16 + child * 12
    mutations = {}
    def variant(name, at, data):
        changed = bytearray(original)
        changed[at:at + len(data)] = data
        mutations[name] = bytes(changed)
    variant('nan_bound', bounds + 6, struct.pack('<H', 0x7e00))
    variant('infinite_bound', bounds + 6, struct.pack('<H', 0x7c00))
    variant('inverted_bound', bounds + 6, struct.pack('<H', 0xfbff))
    variant('clipped_bound', bounds + 6, original[bounds:bounds + 2])
    variant('root_cycle', root_offset + child * 4, struct.pack('<I', root))
    variant('pointer_outside', root_offset + child * 4, struct.pack('<I', 0xfffffff8))
    mutations['truncated'] = original[:-1]
    report = {'scope': 'Offline conservative raw BOX16/BOX32 path containment, rejecting shared leaves and clipped/spatially split trees; not a complete runtime safety proof',
              'serialized_sha256': digest,
              'validator_sha256': hashlib.sha256((ROOT / 'tools/reference/blas/validate_traversal.rs').read_bytes()).hexdigest(),
              'decoder_sha256': hashlib.sha256((ROOT / 'runtime/d3d12/src/commands/serialized_blas.rs').read_bytes()).hexdigest(),
              'mutations': {}, 'summary': ''}
    with TemporaryDirectory(prefix='tuxbox-traversal-bounds-') as directory:
        work = Path(directory)
        control = work / 'validator'
        subprocess.run(['rustc', '--edition=2024', '-O', '-A', 'dead_code',
                        ROOT / 'tools/reference/blas/validate_traversal.rs', '-o', control], check=True)
        def run(data):
            path = work / 'case.serialized'
            path.write_bytes(data)
            return subprocess.run([control, path], text=True, capture_output=True)
        good = run(original)
        if good.returncode:
            raise ValueError(f'original failed: {good.stderr}')
        report['summary'] = good.stdout.strip()
        for name, data in mutations.items():
            result = run(data)
            if result.returncode == 0:
                raise ValueError(f'malformed {name} was accepted')
            report['mutations'][name] = result.stderr.strip().removeprefix('serialized traversal validation failed: ')
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report, indent=2))

if __name__ == '__main__':
    main()
