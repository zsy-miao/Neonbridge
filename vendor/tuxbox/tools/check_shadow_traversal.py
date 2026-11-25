#!/usr/bin/env python3
"""Compare guarded GPU traversal of a synthetic Xbox TLAS and retail BLAS."""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import struct
import subprocess
import sys
from tempfile import TemporaryDirectory

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from build import RUST_STD_LIBS, Toolchain, find_dxc
from check_frosty_blas import HIT, sphere_probes, sphere_results
from check_frosty_serialized_blas import MESHES, read_asset
from project_tuxbox.host import Host


def sha(data):
    return hashlib.sha256(data).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('mount', type=Path)
    parser.add_argument('--sphere-vertices', type=Path, required=True)
    parser.add_argument('--proton', type=Path, required=True)
    parser.add_argument('--prefix-amd', type=Path, required=True)
    parser.add_argument('--prefix-nvidia', type=Path, required=True)
    parser.add_argument('--amd-vulkan', type=Path)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    m = MESHES['sphere']
    asset = read_asset(args.mount, m)
    source = asset[m.blas_offset:m.blas_offset+m.blas_size]
    if sha(source) != '38bd71be9dd00e74952a9612f87f7df37a1617ca9f9847bb531b40bc5a4ad6af':
        raise ValueError('unexpected sphere BLAS')
    folder = ROOT / 'tools/reference/shadow-tlas'
    report = {'scope': 'GPU-written sphere BLAS source, bounded GPU verification and guarded mathematical traversal of a synthetic TLAS; not hardware-exact node instructions or runtime integration',
              'serialized_sha256': sha(source), 'shader_sha256': sha((folder / 'traverse.hlsl').read_bytes()),
              'verification_shader_sha256': sha((folder / 'verified_shadow.hlsl').read_bytes()), 'adapters': {}}
    with TemporaryDirectory(prefix='tuxbox-shadow-traversal-') as directory:
        work = Path(directory)
        (work / 'sphere.serialized').write_bytes(source)
        expected = sphere_probes(args.sphere_vertices, work, True)
        (work / 'rays.bin').write_bytes((work / 'sphere-rays.bin').read_bytes())
        cpu = work / 'cpu'
        subprocess.run(['rustc', '--edition=2024', '-O', '-A', 'dead_code', ROOT / 'tools/reference/blas/traverse_serialized.rs', '-o', cpu], check=True)
        subprocess.run([cpu, work / 'sphere.serialized', work / 'sphere.xyz', '--export-shadow-tlas', work / 'scene.bin'], check=True)
        subprocess.run([cpu, work / 'sphere.serialized', work / 'sphere.xyz', work / 'rays.bin', work / 'cpu-hits.bin', '--shadow-tlas'], check=True)
        cpu_hits = list(HIT.iter_unpack((work / 'cpu-hits.bin').read_bytes()))
        sphere_results(work / 'cpu-hits.bin', expected)
        report['scene_sha256'] = sha((work / 'scene.bin').read_bytes())
        report['rays_sha256'] = sha((work / 'rays.bin').read_bytes())
        subprocess.run(['cargo', 'xwin', 'build', '--manifest-path', folder / 'owned/Cargo.toml', '--target', 'x86_64-pc-windows-msvc',
                        '--target-dir', work / 'cargo', '--release', '--locked'], check=True)
        lib = work / 'cargo/x86_64-pc-windows-msvc/release/tuxbox_owned_shadow_control.lib'
        dxc = Path(find_dxc(None))
        subprocess.run([dxc, '-T', 'cs_6_0', '-E', 'main', '-O3', '-Fh', work / 'traverse_dxil.h',
                        '-Vn', 'traverse_dxil', folder / 'traverse.hlsl'], check=True)
        for stage in ('initialize', 'compare', 'commit', 'finalize'):
            subprocess.run([dxc, '-T', 'cs_6_0', '-E', stage, '-O3', '-Fh', work / f'verified_{stage}_dxil.h',
                            '-Vn', f'verified_{stage}_dxil', folder / 'verified_shadow.hlsl'], check=True)
        compiler = Toolchain()
        obj = compiler.compile(folder / 'traverse.cpp', work, f'/I{ROOT / "runtime/d3d12"}', f'/I{work}')
        exe = work / 'traverse.exe'
        compiler.link(exe, obj, str(lib), 'd3d12.lib', 'dxgi.lib', *RUST_STD_LIBS)
        subprocess.run([dxc, '-T', 'cs_6_5', '-E', 'main', '-O3', '-Fh', work / 'query_dxil.h',
                        '-Vn', 'query_dxil', ROOT / 'tools/reference/blas/query.hlsl'], check=True)
        desktop_obj = compiler.compile(ROOT / 'tools/reference/blas/guest.cpp', work, '/W4', '/WX',
                                       '/DTUXBOX_SHADOW_TLAS_PROBE', f'/I{work}', f'/I{ROOT / "runtime/d3d12"}')
        desktop_exe = work / 'desktop.exe'
        compiler.link(desktop_exe, desktop_obj, 'd3d12.lib', 'dxgi.lib', 'kernel32.lib')
        report['desktop_guest_sha256'] = sha(desktop_exe.read_bytes())
        for name, prefix in [('AMD', args.prefix_amd), ('NVIDIA', args.prefix_nvidia)]:
            os.environ.update(VKD3D_FILTER_DEVICE_NAME=name, DXVK_FILTER_DEVICE_NAME=name)
            if name == 'AMD' and args.amd_vulkan:
                os.environ['VK_DRIVER_FILES'] = str(args.amd_vulkan)
            elif name == 'NVIDIA':
                os.environ.pop('VK_DRIVER_FILES', None)
            host = Host(None, str(args.proton), prefix)
            native = host.run(desktop_exe, ['desktop', 'desktop.txt', 'sphere'], timeout=60, capture=True)
            native_receipt = (work / 'desktop.txt').read_text() if (work / 'desktop.txt').exists() else ''
            if native.returncode:
                raise RuntimeError(f'{name} native RayQuery failed: {native_receipt}')
            native_adapter = next((line.removeprefix('adapter=') for line in native_receipt.splitlines() if line.startswith('adapter=')), None)
            if native_adapter is None or not native_adapter.lower().startswith('1002:' if name == 'AMD' else '10de:'):
                raise ValueError(f'{name} unexpected native adapter {native_adapter!r}')
            native_path = work / 'sphere-hits.bin'
            sphere_results(native_path, expected)
            native_hits = list(HIT.iter_unpack(native_path.read_bytes()))
            result = host.run(exe, [], timeout=45, capture=True)
            receipt = (work / 'result.txt').read_text()
            if result.returncode:
                raise RuntimeError(f'{name} failed: {receipt}')
            adapter = next((line.removeprefix('adapter=') for line in receipt.splitlines() if line.startswith('adapter=')), None)
            if adapter is None or not adapter.lower().startswith('1002:' if name == 'AMD' else '10de:'):
                raise ValueError(f'{name} unexpected adapter {adapter!r}')
            raw = (work / 'hits.bin').read_bytes()
            length = len(expected)*16
            if len(raw) != length*2+84 or raw[length:length*2] != struct.pack('<4I',0xfffffffe,0,0,0)*len(expected):
                raise ValueError(f'{name} guard failure output differs')
            if raw[length*2:length*2+80] != struct.pack('<4I',0xfffffffd,0,0,0)*5 or struct.unpack_from('<I', raw, length*2+80)[0] != 1:
                raise ValueError(f'{name} unsupported ray was not rejected')
            mismatch = (work / 'mismatch.bin').read_bytes()
            if (len(mismatch) != length*2+84 or
                mismatch[:length*2+80] != struct.pack('<4I',0xfffffffe,0,0,0)*(2*len(expected)+5) or
                struct.unpack_from('<I', mismatch, length*2+80)[0] != 2):
                raise ValueError(f'{name} replay did not gate mismatched source')
            (work / 'gpu-hits.bin').write_bytes(raw[:length])
            stats = sphere_results(work / 'gpu-hits.bin', expected)
            gpu_hits = list(HIT.iter_unpack(raw[:length]))
            errors = [abs(a[3]-b[3]) for a,b in zip(gpu_hits,cpu_hits)]
            if any(a[:3]!=b[:3] or not math.isclose(a[3],b[3],abs_tol=0.0002,rel_tol=0) for a,b in zip(gpu_hits,cpu_hits)):
                raise ValueError(f'{name} CPU reference mismatch')
            if len(native_hits) != len(gpu_hits) or any(a[:3]!=b[:3] or not math.isclose(a[3],b[3],abs_tol=0.0002,rel_tol=0) for a,b in zip(gpu_hits,native_hits)):
                raise ValueError(f'{name} native RayQuery mismatch')
            native_errors = [abs(a[3]-b[3]) for a,b in zip(gpu_hits,native_hits)]
            report['adapters'][name] = {'adapter':adapter, **stats, 'cpu_max_distance_error': max(errors),
                                         'native_rayquery_max_distance_error': max(native_errors),
                                         'guard_rejections':len(expected), 'unsupported_ray_rejections':5,
                                         'gpu_verified_mismatch_replay':True}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2)+'\n')
    print(json.dumps(report,indent=2))

if __name__ == '__main__':
    main()
