#!/usr/bin/env python3
"""GPU-test an isolated Rust-owned validated immutable BLAS snapshot."""
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
from build import RUST_STD_LIBS, Toolchain, find_dxc
from check_frosty_serialized_blas import MESHES, read_asset
from project_tuxbox.host import Host


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('mount', type=Path)
    parser.add_argument('--proton', type=Path, required=True)
    parser.add_argument('--prefix-amd', type=Path, required=True)
    parser.add_argument('--prefix-nvidia', type=Path, required=True)
    parser.add_argument('--amd-vulkan', type=Path)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    mesh = MESHES['sphere']
    asset = read_asset(args.mount, mesh)
    source = asset[mesh.blas_offset:mesh.blas_offset + mesh.blas_size]
    digest = hashlib.sha256(source).hexdigest()
    if digest != '38bd71be9dd00e74952a9612f87f7df37a1617ca9f9847bb531b40bc5a4ad6af':
        raise ValueError('unexpected serialized sphere')
    folder = ROOT / 'tools/reference/shadow-tlas'
    report = {'scope': 'Isolated Rust-owned immutable upload and default-heap GPU copy with private-fence retention; not runtime queue-ordered source capture or shader integration',
              'serialized_sha256': digest,
              'rust_sha256': hashlib.sha256((folder / 'owned/src/lib.rs').read_bytes()).hexdigest(),
              'validator_sha256': hashlib.sha256((ROOT / 'tools/reference/blas/validate_traversal.rs').read_bytes()).hexdigest(),
              'shader_sha256': hashlib.sha256((folder / 'owned_shadow.hlsl').read_bytes()).hexdigest(),
              'verification_shader_sha256': hashlib.sha256((folder / 'verified_shadow.hlsl').read_bytes()).hexdigest(),
              'guarded_consumer_sha256': hashlib.sha256((folder / 'guarded_shadow.hlsl').read_bytes()).hexdigest(),
              'adapters': {}}
    with TemporaryDirectory(prefix='tuxbox-owned-shadow-') as directory:
        work = Path(directory)
        (work / 'source.bin').write_bytes(source)
        subprocess.run(['cargo', 'xwin', 'build', '--manifest-path', folder / 'owned/Cargo.toml',
                        '--target', 'x86_64-pc-windows-msvc', '--target-dir', work / 'cargo',
                        '--release', '--locked'], check=True)
        library = work / 'cargo/x86_64-pc-windows-msvc/release/tuxbox_owned_shadow_control.lib'
        dxc = Path(find_dxc(None))
        subprocess.run([dxc, '-T', 'cs_6_0', '-E', 'main', '-O3', '-Fh', work / 'owned_shadow_dxil.h',
                        '-Vn', 'owned_shadow_dxil', folder / 'owned_shadow.hlsl'], check=True)
        for stage in ('initialize', 'compare', 'commit', 'finalize'):
            subprocess.run([dxc, '-T', 'cs_6_0', '-E', stage, '-O3', '-Fh', work / f'verified_{stage}_dxil.h',
                            '-Vn', f'verified_{stage}_dxil', folder / 'verified_shadow.hlsl'], check=True)
        subprocess.run([dxc, '-T', 'cs_6_0', '-E', 'main', '-O3', '-Fh', work / 'guarded_shadow_dxil.h',
                        '-Vn', 'guarded_shadow_dxil', folder / 'guarded_shadow.hlsl'], check=True)
        compiler = Toolchain()
        obj = compiler.compile(folder / 'owned_shadow.cpp', work, f'/I{ROOT / "runtime/d3d12"}', f'/I{work}')
        exe = work / 'owned.exe'
        compiler.link(exe, obj, str(library), 'd3d12.lib', 'dxgi.lib', *RUST_STD_LIBS)
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
            required = ('immutable_gpu_snapshot=1', 'malformed_rejected=1', 'blocked_gpu_retained=1', 'two_fence_retirement=1', 'two_gpu_queues=1', 'default_heap_gpu_copy=1', 'completed_list_copy_replay=1', 'closed_list_retention=1', 'inflight_copy_after_list_reset=1', 'validated_source_update=1', 'inflight_update_rejected=1', 'gpu_written_source_verified=1', 'last_dword_mismatch_preserves_shadow=1', 'guarded_consumer_rejects_mismatch=1', 'bounded_views_reject_invalid_sizes=1', 'parallel_verification=1', 'large_parallel_body=131072', 'large_last_dword_mismatch=1', 'multidimensional_dispatch=1', 'underdispatch_rejected=1')
            if any(item not in receipt.splitlines() for item in required):
                raise ValueError(f'{name} missing control receipt: {receipt}')
            report['adapters'][name] = {'adapter': adapter, 'immutable_after_source_mutation': True,
                                         'malformed_rejected': True, 'blocked_gpu_retained': True,
                                         'two_fence_retirement': True, 'two_gpu_queues': True, 'default_heap_gpu_copy': True, 'completed_list_copy_replay': True, 'closed_list_retention': True, 'inflight_copy_after_list_reset': True, 'validated_source_update': True, 'inflight_update_rejected': True, 'gpu_written_source_verified': True, 'last_dword_mismatch_preserves_shadow': True, 'guarded_consumer_rejects_mismatch': True, 'bounded_views_reject_invalid_sizes': True, 'parallel_verification': True, 'large_parallel_body_bytes': 131072, 'large_last_dword_mismatch': True, 'multidimensional_dispatch': True, 'underdispatch_rejected': True}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report, indent=2))

if __name__ == '__main__':
    main()
