#!/usr/bin/env python3
"""Replay XG descriptors recorded by the research proxy without bundling title data."""
import argparse
from collections import Counter
import json
import os
from pathlib import Path
import re
import shutil
import struct
import subprocess
import sys
from tempfile import TemporaryDirectory

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from build import Toolchain
from project_tuxbox.host import proton_installation
from project_tuxbox.inspect import digest

PATTERN = re.compile(r'XG_REFERENCE_TRACE call=(\d+) hr=([0-9a-fA-F]+) dimension=(\d+) alignment=(\d+) width=(\d+) height=(\d+) slices=(\d+) mips=(\d+) format=(\d+) samples=(\d+) quality=(\d+) layout=(\d+) flags=([0-9a-fA-F]+)')
DESC = struct.Struct('<I4xQQIHHIIIII4x')
RECORD_SIZE = 0x1b60  # Native struct includes x64 alignment padding.


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--log', required=True, type=Path)
    parser.add_argument('--original-xg', required=True, type=Path)
    parser.add_argument('--provider', required=True, type=Path)
    parser.add_argument('--xmem', required=True, type=Path)
    parser.add_argument('--proton', required=True, type=Path)
    parser.add_argument('--prefix', required=True, type=Path)
    parser.add_argument('--output', required=True, type=Path)
    args = parser.parse_args()
    matches = {}
    for m in PATTERN.finditer(args.log.read_text(errors='replace')):
        call, hr, *fields = m.groups()
        matches[int(call)] = (int(hr, 16), *(int(value, 16) if i == 10 else int(value) for i, value in enumerate(fields)))
    # Only successful reference descriptors can establish an implementation target.
    descs = sorted({fields[1:] for fields in matches.values() if fields[0] == 0})
    if not descs:
        raise ValueError('No successful reference descriptors found')
    data = b''.join(DESC.pack(*d) for d in descs)
    env = os.environ.copy()
    env.update(WINEPREFIX=str(args.prefix.resolve()), WINEDEBUG='-all', WINEDLLOVERRIDES='xg_subject,xmem=n')
    wine = proton_installation(args.proton) / 'files/bin/wine'
    with TemporaryDirectory(prefix='xg-trace-replay-') as tmp:
        work=Path(tmp)
        tools=Toolchain()
        obj=tools.compile((ROOT/'runtime/xg/trace_replay_control.cpp').resolve(),work,'/W4','/WX')
        exe=work/'trace-replay.exe'
        tools.link(exe,obj,'kernel32.lib')
        outputs=[]
        for name,dll in [('original',args.original_xg),('tuxbox',args.provider)]:
            trial=work/name; trial.mkdir()
            shutil.copy2(exe,trial/exe.name); shutil.copy2(dll,trial/'xg_subject.dll'); shutil.copy2(args.xmem,trial/'xmem.dll')
            (trial/'descriptors.bin').write_bytes(data)
            run=subprocess.run([str(wine),str(trial/exe.name)],cwd=trial,env=env,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,timeout=90)
            if run.returncode: raise RuntimeError(f'{name} guest failed: {run.stdout.decode(errors="replace")}')
            raw=(trial/'results.bin').read_bytes()
            if len(raw)!=RECORD_SIZE*len(descs): raise ValueError(f'{name}: result length {len(raw)} does not match record size')
            outputs.append(raw)
        unsupported=Counter(); unsupported_descriptors=[]; mismatches=[]; matched=0; original_failures=0
        for i,d in enumerate(descs):
            left=outputs[0][i*RECORD_SIZE:(i+1)*RECORD_SIZE];right=outputs[1][i*RECORD_SIZE:(i+1)*RECORD_SIZE]
            a=struct.unpack_from('<I',left)[0];b=struct.unpack_from('<I',right)[0]
            if a:
                original_failures+=1
                continue
            if b:
                unsupported[(d[0],d[-1],d[5],d[4])]+=1
                unsupported_descriptors.append({'dimension':d[0], 'alignment':d[1], 'width':d[2], 'height':d[3], 'slices':d[4], 'mips':d[5], 'format':d[6], 'samples':d[7], 'quality':d[8], 'layout':d[9], 'flags':hex(d[10]), 'provider_hresult':hex(b)})
            elif left!=right: mismatches.append({'descriptor':d,'first_mismatch':next(k for k,(x,y) in enumerate(zip(left,right)) if x!=y)})
            else: matched+=1
        result={'scope':'Original and TuxBox XG replay of successful descriptors captured by the research proxy; compares complete layouts and first/last mip-slice origin addresses for each supported case. Numeric descriptor values and file hashes only; the game and its binaries are not included.',
                'trace_sha256':digest(args.log),'original_xg_sha256':digest(args.original_xg),'provider_sha256':digest(args.provider),'xmem_sha256':digest(args.xmem),
                'guest_source_sha256':digest(ROOT/'runtime/xg/trace_replay_control.cpp'),'guest_sha256':digest(exe),
                'captured_calls':len(matches),'successful_unique_descriptors':len(descs),'original_replay_failures':original_failures,'matched_layouts_and_samples':matched,
                'unsupported_unique_descriptors':sum(unsupported.values()),
                'unsupported_descriptors':unsupported_descriptors,
                'unsupported_families':[{'dimension':key[0],'flags':hex(key[1]),'mips':key[2],'slices':key[3],'unique_descriptors':value} for key,value in unsupported.most_common()],
                'mismatched_supported':mismatches}
        args.output.parent.mkdir(parents=True,exist_ok=True)
        args.output.write_text(json.dumps(result,indent=2)+'\n')
        print(json.dumps(result,indent=2))


if __name__=='__main__': main()
