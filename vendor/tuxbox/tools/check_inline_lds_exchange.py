#!/usr/bin/env python3
"""Validate a real shader-219 LDS exchange instruction in a synthetic GPU control."""
import argparse
import hashlib
import json
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
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('shader', type=Path)
    p.add_argument('--proton', type=Path, required=True)
    p.add_argument('--prefix-amd', type=Path, required=True)
    p.add_argument('--prefix-nvidia', type=Path, required=True)
    p.add_argument('--amd-vulkan', type=Path)
    p.add_argument('--output', type=Path, required=True)
    a = p.parse_args()
    report = {'scope': 'One real 219 LDS exchange in a synthetic 32-lane group; no full shader, traversal or wave control claim', 'shader_sha256': hashlib.sha256(a.shader.read_bytes()).hexdigest(), 'adapters': {}}
    with TemporaryDirectory(prefix='tuxbox-lds-exchange-') as temp:
        w = Path(temp)
        subprocess.run(['cargo','build','--manifest-path',ROOT/'tools/shader-compiler/Cargo.toml','--target-dir',w/'cargo','--target','x86_64-unknown-linux-gnu','--lib'], check=True)
        deps = w/'cargo/x86_64-unknown-linux-gnu/debug/deps'
        lib = next(deps.glob('libtuxbox_xshader-*.rlib'))
        gen = w/'extract'
        subprocess.run(['rustc','--edition=2021',ROOT/'tools/reference/blas/extract_lds_exchange.rs','--extern',f'tuxbox_xshader={lib}','-L',f'dependency={deps}','-o',gen], check=True)
        hlsl = w/'probe.hlsl'
        subprocess.run([gen,a.shader,hlsl],check=True)
        dxc=Path(find_dxc(None)); cso=w/'probe.cso'
        subprocess.run([dxc,'-T','cs_6_0','-E','main','-Gis',hlsl,'-Fo',cso],check=True)
        subprocess.run([dxc.with_name('dxv'),cso],check=True)
        report['hlsl_sha256']=hashlib.sha256(hlsl.read_bytes()).hexdigest()
        tc=Toolchain(); obj=tc.compile(ROOT/'tools/reference/independent-shaders/compute.cpp',w,f'/I{ROOT / "runtime/d3d12"}')
        exe=w/'probe.exe'; tc.link(exe,obj,'d3d12.lib','dxgi.lib','kernel32.lib')
        seeds={}
        for mode in ('unique','inactive','collision'):
            seed=[0x76540000+i for i in range(384)]
            for lane in range(32):
                idx=96+lane*4; addr=lane%4 if mode=='collision' else lane
                seed[idx:idx+4]=[addr*4,0x2000+lane,0x1000+lane, int(mode != 'inactive' or lane%3 != 0)]
            seeds[mode]=seed
            (w/f'{mode}.seed').write_bytes(struct.pack('<384I',*seed))
            (w/f'{mode}.cso').write_bytes(cso.read_bytes())
        (w/'cases.txt').write_text(''.join(f'{s} 0 uint 1\n' for s in seeds))
        for name,prefix in [('AMD',a.prefix_amd),('NVIDIA',a.prefix_nvidia)]:
            os.environ.update(VKD3D_FILTER_DEVICE_NAME=name,DXVK_FILTER_DEVICE_NAME=name)
            if name == 'AMD' and a.amd_vulkan: os.environ['VK_DRIVER_FILES']=str(a.amd_vulkan)
            else: os.environ.pop('VK_DRIVER_FILES',None)
            r=Host(None,str(a.proton),prefix).run(exe,[],timeout=180,capture=True)
            if r.returncode: raise RuntimeError(f'{name} guest failed: {(w/"result.txt").read_text()}')
            receipt=(w/'result.txt').read_text()
            adapter=next(x[8:] for x in receipt.splitlines() if x.startswith('adapter='))
            if not adapter.lower().startswith('1002:' if name == 'AMD' else '10de:'): raise ValueError('wrong GPU')
            for mode,seed in seeds.items():
                got=struct.unpack('<384I',(w/f'{mode}.bin').read_bytes())
                for i in range(384):
                    if not 96 <= i < 224 or (i-96)%4 in (0,3):
                        if got[i]!=seed[i]: raise ValueError(f'{name} {mode} guard/input {i}')
                for slot in range(32):
                    updates=[lane for lane in range(32) if seed[96+lane*4]//4==slot and seed[96+lane*4+3]]
                    final=got[96+slot*4+2]
                    expected=[0x1000+slot]+[0x2000+lane for lane in updates]
                    if final not in expected: raise ValueError(f'{name} {mode} invalid final {slot}')
                    expected.remove(final)
                    previous=[got[96+lane*4+1] for lane in updates]
                    if sorted(previous)!=sorted(expected): raise ValueError(f'{name} {mode} invalid atomic history {slot}')
                for lane in range(32):
                    if not seed[96+lane*4+3] and got[96+lane*4+1] != seed[96+lane*4+1]: raise ValueError(f'{name} inactive lane {lane}')
            report['adapters'][name]={'adapter':adapter,'cases':list(seeds),'lanes_per_case':32,'mismatches':0}
    a.output.parent.mkdir(parents=True,exist_ok=True); a.output.write_text(json.dumps(report,indent=2)+'\n')
    print('LDS exchange: both GPUs passed unique, inactive and colliding cases')

if __name__ == '__main__': main()
