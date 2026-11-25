#!/usr/bin/env python3
"""Check retained raw Xbox BLAS copies across two ordered GPU queues."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
from tempfile import TemporaryDirectory

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from build import Toolchain, find_dxc
from check_frosty_serialized_blas import MESHES, read_asset
from project_tuxbox.host import Host


def digest(data):
    return hashlib.sha256(data).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('mount', type=Path)
    parser.add_argument('--proton', type=Path, required=True)
    parser.add_argument('--prefix-amd', type=Path, required=True)
    parser.add_argument('--prefix-nvidia', type=Path, required=True)
    parser.add_argument('--amd-vulkan', type=Path)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    sphere = MESHES['sphere']
    asset = read_asset(args.mount, sphere)
    source = asset[sphere.blas_offset:sphere.blas_offset+sphere.blas_size]
    if len(source) <= 64 or digest(source) != '38bd71be9dd00e74952a9612f87f7df37a1617ca9f9847bb531b40bc5a4ad6af':
        raise ValueError('unexpected serialized sphere')
    body = source[64:]
    modified = body[:-1] + bytes([body[-1] ^ 0x5a])
    report = {'scope': 'Standalone retained-body and five-entry GPU directory controls; not runtime BLAS provenance or compact-AS implementation',
              'directory_checks': ['unique matches', 'missing key', 'duplicate key', 'invalid guest addresses', 'invalid backing addresses', 'zero and oversized counts', 'cross-queue ordering', 'ordered rewrite'],
              'serialized_sha256': digest(source), 'body_sha256': digest(body),
              'modified_body_sha256': digest(modified), 'body_bytes': len(body),
              'shader_sha256': digest((ROOT / 'tools/reference/shadow-tlas/retained_blas.hlsl').read_bytes()),
              'directory_shader_sha256': digest((ROOT / 'tools/reference/shadow-tlas/directory.hlsl').read_bytes()), 'adapters': {}}
    with TemporaryDirectory(prefix='tuxbox-retained-blas-') as directory:
        work = Path(directory)
        (work / 'source.bin').write_bytes(source)
        dxc = Path(find_dxc(None))
        subprocess.run([dxc, '-T', 'cs_6_0', '-E', 'main', '-O3', '-Fh', work / 'retained_dxil.h',
                        '-Vn', 'retained_dxil', ROOT / 'tools/reference/shadow-tlas/retained_blas.hlsl'], check=True)
        subprocess.run([dxc, '-T', 'cs_6_0', '-E', 'main', '-O3', '-Fh', work / 'directory_dxil.h',
                        '-Vn', 'directory_dxil', ROOT / 'tools/reference/shadow-tlas/directory.hlsl'], check=True)
        toolchain = Toolchain()
        obj = toolchain.compile(ROOT / 'tools/reference/shadow-tlas/retained_blas.cpp', work, f'/I{ROOT / "runtime/d3d12"}', f'/I{work}')
        exe = work / 'retained.exe'
        toolchain.link(exe, obj, 'd3d12.lib', 'dxgi.lib', 'kernel32.lib')
        for name, prefix in [('AMD', args.prefix_amd), ('NVIDIA', args.prefix_nvidia)]:
            os.environ.update(VKD3D_FILTER_DEVICE_NAME=name, DXVK_FILTER_DEVICE_NAME=name)
            if name == 'AMD' and args.amd_vulkan:
                os.environ['VK_DRIVER_FILES'] = str(args.amd_vulkan)
            elif name == 'NVIDIA':
                os.environ.pop('VK_DRIVER_FILES', None)
            result = Host(None, str(args.proton), prefix).run(exe, [], timeout=45, capture=True)
            receipt = (work / 'result.txt').read_text()
            if result.returncode:
                raise RuntimeError(f'{name} GPU control failed: {receipt}')
            adapter = next((line.removeprefix('adapter=') for line in receipt.splitlines() if line.startswith('adapter=')), None)
            if adapter is None or not adapter.lower().startswith('1002:' if name == 'AMD' else '10de:'):
                raise ValueError(f'{name} unexpected adapter {adapter!r}')
            if (work / 'retained.bin').read_bytes() != body + modified + body + body[:64] + modified[:64] + body[:64]:
                raise ValueError(f'{name} retained copies differ')
            if (work / 'replayed.bin').read_bytes() != modified + body + modified + modified[:64] + body[:64] + modified[:64]:
                raise ValueError(f'{name} replayed copies differ')
            if (work / 'reversed.bin').read_bytes() != modified:
                raise ValueError(f'{name} reversed submission order differs')
            report['adapters'][name] = {'adapter': adapter, 'source_offset': 320, 'copies': 3,
                                       'two_queues_with_fence': True, 'blocked_fence_verified': True, 'upload_written_after_close': True, 'gpu_header_reads': 3, 'replayed_after_completion': True, 'reversed_list_submission': True, 'gpu_directory_publication': True, 'directory_descriptor_bounded_views': True, 'directory_entries': 5, 'directory_queries': 44, 'directory_short_output_guard': True, 'directory_unaligned_backing': True, 'directory_cross_queue': True, 'directory_zero_query_guard': True, 'directory_replay_updated_entry': True}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report, indent=2))

if __name__ == '__main__':
    main()
