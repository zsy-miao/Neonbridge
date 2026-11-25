#!/usr/bin/env python3
"""Compare bounded GPU guest-address lookup against an independent CPU oracle."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import random
import struct
import subprocess
import sys
from tempfile import TemporaryDirectory

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from build import Toolchain, find_dxc
from project_tuxbox.host import Host

MAX = (1 << 64) - 1
SLOTS = [list(range(0x11100000, 0x11100010)), list(range(0x22200000, 0x22200010)), list(range(0x33300000, 0x33300010))]
BASES = [0x00100000, 0x00000001fffffff0, 0xfffffffffffff000]


def query(address, width=4, reserved=0):
    return (address & 0xffffffff, address >> 32, width, reserved)


def entry(address, length, slot):
    return (address & 0xffffffff, address >> 32, length, slot)


def oracle(entries, queries, declared_entries, declared_queries, output_capacity):
    active = min(declared_queries, output_capacity)
    if declared_entries > 3 or declared_entries > len(entries) or declared_queries > len(queries) or declared_queries > output_capacity:
        return [(0,0,0,0,3)] * active
    spans = []
    for lo, hi, length, slot in entries[:declared_entries]:
        base = lo | hi << 32
        if not base or base % 4 or not length or length % 4 or slot >= 3 or length > len(SLOTS[slot])*4 or base + length > MAX:
            return [(0,0,0,0,3)] * active
        for previous_base, previous_end, previous_slot in spans:
            if slot == previous_slot or (base < previous_end and previous_base < base + length):
                return [(0,0,0,0,2)] * active
        spans.append((base, base + length, slot))
    output = []
    for lo, hi, width, reserved in queries[:active]:
        address = lo | hi << 32
        if not address or address % 4 or reserved or width not in (4,8,12,16):
            output.append((0,0,0,0,3)); continue
        found = None
        for base, end, slot in spans:
            if base <= address and address + width <= end:
                data = SLOTS[slot][(address-base)//4:(address-base+width)//4]
                found = tuple(data + [0]*(4-len(data))) + (1,)
                break
        output.append(found if found is not None else (0,0,0,0,0))
    return output


def cases():
    good = [entry(BASES[i],64,i) for i in range(3)]
    q = [query(base + offset, width) for base in BASES for offset in (0,4,48,52,60,64)
         for width in (4,8,12,16)]
    q += [query(0),query(0x100001),query(0x100000,3),query(0x100000,20),query(0x100000,4,1),
          query(0xfffffffffffffffc,8), query(0x0000000200000000,16)]
    rng = random.Random(2192026)
    q += [query(rng.choice(BASES) + rng.randrange(0,24)*4,rng.choice((4,8,12,16))) for _ in range(256)]
    yield 'valid_and_random', good, q, 3, len(q), len(q)
    yield 'empty', [], q[:8], 0, 8, 8
    yield 'adjacent_ranges', [good[0],entry(BASES[0]+64,64,1)], [query(BASES[0]+60),query(BASES[0]+64),query(BASES[0]+60,8)], 2, 3, 3
    yield 'overlap', [good[0],entry(BASES[0]+16,64,1)], q[:8], 2, 8, 8
    yield 'aliased_slot', [good[0],entry(BASES[1],64,0)], q[:8], 2, 8, 8
    bad = [('null_base',entry(0,64,0)), ('unaligned_base',entry(0x100001,64,0)),
           ('zero_length',entry(0x100000,0,0)), ('unaligned_length',entry(0x100000,7,0)),
           ('address_overflow',entry(0xfffffffffffffffc,8,0)), ('invalid_slot',entry(0x100000,64,3)),
           ('short_backing',entry(0x100000,68,0))]
    for label, value in bad:
        yield label, [value], q[:8], 1, 8, 8
    yield 'too_many_entries', good, q[:8], 4, 8, 8
    yield 'short_directory', good[:2], q[:8], 3, 8, 8
    yield 'short_queries', good, q[:4], 3, 8, 8
    yield 'short_output', good, q[:8], 3, 8, 4


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--proton', type=Path, required=True)
    p.add_argument('--prefix-amd', type=Path, required=True)
    p.add_argument('--prefix-nvidia', type=Path, required=True)
    p.add_argument('--amd-vulkan', type=Path)
    p.add_argument('--output', type=Path, required=True)
    args = p.parse_args()
    src = ROOT / 'tools/reference/flat-address'
    report = {'scope':'Standalone bounded 64-bit guest-address lookup for aligned DWORD vectors; no runtime guest VA translation',
              'limitations':['Caller must prove descriptor allocation identity, physical non-aliasing, lifetime and queue synchronization.',
                             'Unaligned accesses, stores and atomics are unsupported; no claim about native 219 alignment or VA width.'],
              'shader_sha256':hashlib.sha256((src/'lookup.hlsl').read_bytes()).hexdigest(),
              'registry_sha256':hashlib.sha256((src/'registry.rs').read_bytes()).hexdigest(), 'adapters':{}}
    all_cases = list(cases())
    with TemporaryDirectory(prefix='tuxbox-flat-address-') as directory:
        work = Path(directory)
        registry = work/'registry'
        subprocess.run(['rustc','--edition=2024','-O',src/'registry_cli.rs','-o',registry],check=True)
        for label, entries, _, _, _, _ in all_cases:
            if label not in ('valid_and_random', 'adjacent_ranges'):
                continue
            records = b''.join(struct.pack('<QIIQQQ', lo | hi << 32, length, slot, slot + 1, 0, 64)
                               for lo, hi, length, slot in entries)
            encoded = subprocess.run([registry],input=records,stdout=subprocess.PIPE,check=True).stdout
            if encoded != b''.join(struct.pack('<4I',*entry) for entry in entries):
                raise ValueError(f'Rust registry layout mismatch for {label}')
        dxc = Path(find_dxc(None))
        subprocess.run([dxc,'-T','cs_6_0','-E','main','-O3','-Fo',work/'lookup.cso',src/'lookup.hlsl'],check=True)
        subprocess.run([dxc.with_name('dxv'),work/'lookup.cso'],check=True)
        report['dxil_sha256']=hashlib.sha256((work/'lookup.cso').read_bytes()).hexdigest()
        toolchain=Toolchain()
        obj=toolchain.compile(src/'lookup.cpp',work,f'/I{ROOT / "runtime/d3d12"}')
        exe=work/'lookup.exe'; toolchain.link(exe,obj,'d3d12.lib','dxgi.lib','kernel32.lib')
        for name,prefix in [('AMD',args.prefix_amd),('NVIDIA',args.prefix_nvidia)]:
            os.environ.update(VKD3D_FILTER_DEVICE_NAME=name,DXVK_FILTER_DEVICE_NAME=name)
            if name=='AMD' and args.amd_vulkan: os.environ['VK_DRIVER_FILES']=str(args.amd_vulkan)
            elif name=='NVIDIA': os.environ.pop('VK_DRIVER_FILES',None)
            host=Host(None,str(args.proton),prefix)
            for i,slot in enumerate(SLOTS): (work/f'data{i}.bin').write_bytes(struct.pack('<16I',*slot))
            summaries={}
            for label, entries, queries, ne, nq, cap in all_cases:
                (work/'directory.bin').write_bytes(b''.join(struct.pack('<4I',*e) for e in entries) or bytes(16))
                (work/'queries.bin').write_bytes(b''.join(struct.pack('<4I',*q) for q in queries) or bytes(16))
                (work/'config.bin').write_bytes(struct.pack('<3I',ne,nq,cap))
                result=host.run(exe,[],timeout=45,capture=True)
                receipt=(work/'result.txt').read_text()
                if result.returncode: raise RuntimeError(f'{name} {label}: {receipt}')
                adapter=next((x.removeprefix('adapter=') for x in receipt.splitlines() if x.startswith('adapter=')),None)
                if adapter is None or not adapter.lower().startswith('1002:' if name=='AMD' else '10de:'):
                    raise ValueError(f'{name} selected unexpected adapter {adapter!r}')
                raw=(work/'lookup.bin').read_bytes()
                actual=struct.unpack(f'<{32+cap*5}I',raw)
                want=oracle(entries,queries,ne,nq,cap)
                expected=[0x76540000+i for i in range(32+cap*5)]
                for idx,row in enumerate(want): expected[16+idx*5:16+idx*5+5]=row
                if list(actual)!=expected:
                    mismatch=next(i for i,(a,b) in enumerate(zip(actual,expected)) if a!=b)
                    raise ValueError(f'{name} {label} word {mismatch}: {actual[mismatch]:08x} != {expected[mismatch]:08x}')
                summaries[label]={'queries_checked':len(want),'guard_words':32}
            report['adapters'][name]={'adapter':adapter,'cases':summaries}
    args.output.parent.mkdir(parents=True,exist_ok=True)
    args.output.write_text(json.dumps(report,indent=2)+'\n')
    print(json.dumps(report,indent=2))

if __name__=='__main__':
    main()
