#!/usr/bin/env python3
"""Compare an experimental GPU instance encoder with the Rust CPU reference."""
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
from build import Toolchain, find_dxc
from project_tuxbox.host import Host


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--proton', type=Path, required=True)
    parser.add_argument('--prefix-amd', type=Path, required=True)
    parser.add_argument('--prefix-nvidia', type=Path, required=True)
    parser.add_argument('--amd-vulkan', type=Path)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    report = {'scope': 'GPU-buffer instance encoding and conservative bounds compared with the independent Rust CPU reference; not runtime TLAS construction', 'adapters': {}}
    with TemporaryDirectory(prefix='tuxbox-shadow-instance-') as directory:
        work = Path(directory)
        reference = work / 'reference'
        subprocess.run(['rustc', '--edition=2024', '-A', 'dead_code', '-O', ROOT / 'tools/reference/shadow-tlas/instance_cases.rs', '-o', reference], check=True)
        lines = subprocess.check_output([reference], text=True).splitlines()
        cases = [[int(n, 16) for n in line.split()] for line in lines]
        if len(cases) < 10 or len(cases) % 2 or any(len(case) != 55 for case in cases):
            raise ValueError('unexpected CPU fixture')
        source = (ROOT / 'tools/reference/shadow-tlas/instance.hlsl').read_text()
        dxc = Path(find_dxc(None))
        report['shaders'] = []
        seeds = []
        for batch in range(len(cases) // 2):
            shader = source + '\nRWBuffer<uint> output : register(u0);\n[numthreads(1,1,1)] void main() {\n'
            seed = [0x76540000 + i for i in range(384)]
            for slot, case in enumerate(cases[batch * 2:batch * 2 + 2]):
                index = batch * 2 + slot
                seed[192+16*slot:192+16*(slot+1)] = case[:16]
                shader += f'{{ uint src[16]; for (uint j = 0; j < 16; ++j) src[j] = output[{96+16*slot}+j]; uint dst[32]; uint status = tb_encode_instance(src, 0xa94u, 0x007f7f7fu, {index}u, 0x33u, dst); float3 lo = 0, hi = 0; if (status == 1 && tb_instance_bounds(src, -1.0f, 1.0f, lo, hi) != 1) status = 2; output[{39*slot}] = status;'
                shader += ''.join(f' output[{39*slot+1+i}] = dst[{i}];' for i in range(32))
                shader += ''.join(f' output[{39*slot+33+i}] = asuint({"lo" if i < 3 else "hi"}[{i%3}]);' for i in range(6)) + ' }\n'
            # Negative controls exercise GPU buffer input validation separately
            # from instance validity, including denormals and arithmetic overflow.
            shader += '{ uint bad[16]; for (uint j=0; j<16; ++j) bad[j]=output[96+j]; float3 lo, hi;'
            shader += ' output[78]=tb_instance_bounds(bad, 1.0f, -1.0f, lo, hi);'
            shader += ' output[79]=tb_instance_bounds(bad, asfloat(1u), 1.0f, lo, hi);'
            shader += ' output[80]=tb_instance_bounds(bad, asfloat(0x7fc00000u), 1.0f, lo, hi);'
            shader += ' bad[0]=0x7f7fffffu; output[81]=tb_instance_bounds(bad, -2.0f, 2.0f, lo, hi); }\n'
            shader += '}\n'
            hlsl, cso = work / f'instance{batch}.hlsl', work / f'instance{batch}.cso'
            hlsl.write_text(shader)
            subprocess.run([dxc, '-T', 'cs_6_0', '-E', 'main', hlsl, '-Fo', cso], check=True)
            subprocess.run([dxc.with_name('dxv'), cso], check=True)
            report['shaders'].append({'hlsl_sha256': hashlib.sha256(shader.encode()).hexdigest(), 'dxil_sha256': hashlib.sha256(cso.read_bytes()).hexdigest()})
            seeds.append(seed)
            (work / f'instance{batch}.seed').write_bytes(struct.pack('<384I', *seed))
        toolchain = Toolchain()
        obj = toolchain.compile(ROOT / 'tools/reference/independent-shaders/compute.cpp', work, f'/I{ROOT / "runtime/d3d12"}')
        exe = work / 'instance.exe'
        toolchain.link(exe, obj, 'd3d12.lib', 'dxgi.lib', 'kernel32.lib')
        (work / 'cases.txt').write_text(''.join(f'instance{i} 0 uint 1\n' for i in range(len(cases) // 2)))
        for name, prefix in [('AMD', args.prefix_amd), ('NVIDIA', args.prefix_nvidia)]:
            os.environ.update(VKD3D_FILTER_DEVICE_NAME=name, DXVK_FILTER_DEVICE_NAME=name)
            if name == 'AMD' and args.amd_vulkan:
                os.environ['VK_DRIVER_FILES'] = str(args.amd_vulkan)
            elif name == 'NVIDIA':
                os.environ.pop('VK_DRIVER_FILES', None)
            result = Host(None, str(args.proton), prefix).run(exe, [], timeout=45, capture=True)
            if result.returncode:
                raise RuntimeError(f'{name} failed: {result.returncode}')
            receipt = (work / 'result.txt').read_text()
            adapter = next((line.removeprefix('adapter=') for line in receipt.splitlines() if line.startswith('adapter=')), None)
            if adapter is None or not adapter.lower().startswith('1002:' if name == 'AMD' else '10de:'):
                raise ValueError(f'{name} selected unexpected adapter {adapter!r}')
            for batch in range(len(cases) // 2):
                data = (work / f'instance{batch}.bin').read_bytes()
                actual = struct.unpack('<384I', data)
                for slot, case in enumerate(cases[batch * 2:batch * 2 + 2]):
                    got = actual[96 + 39*slot:96 + 39*(slot+1)]
                    expected = case[16:]
                    for field, (a, b) in enumerate(zip(got, expected)):
                        if 17 <= field < 29 and expected[0] == 1:
                            a, b = (struct.unpack('<f', struct.pack('<I', x))[0] for x in (a, b))
                            if math.isclose(a, b, rel_tol=1e-6, abs_tol=1e-6):
                                continue
                        elif field >= 33 and expected[0] == 1:
                            a, b = (struct.unpack('<f', struct.pack('<I', x))[0] for x in (a, b))
                            if math.isfinite(a) and ((field < 36 and a <= b) or (field >= 36 and a >= b)):
                                continue
                        elif a == b:
                            continue
                        raise ValueError(f'{name}: case {batch*2+slot} field {field}: {a!r} != {b!r}')
                if actual[96+78:96+82] != (2, 2, 2, 2):
                    raise ValueError(f'{name} invalid GPU bounds accepted: {actual[96+78:96+82]}')
                if any(v != seeds[batch][i] for i, v in enumerate(actual) if i < 96 or i >= 96 + 82):
                    raise ValueError(f'{name} output guard or GPU input changed')
            report['adapters'][name] = {'adapter': adapter, 'cases': len(cases), 'bounds_negative_cases_per_batch': 4, 'guard_words_per_batch': len(actual) - 82}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report, indent=2))

if __name__ == '__main__':
    main()
