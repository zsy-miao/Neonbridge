#!/usr/bin/env python3
"""Compare XGValidateDepthStencilSwizzleMode with the supplied Xbox library.

The same guest calls the Xbox DLL and TuxBox provider with 6,125 combinations
of format, sample count and swizzle mode. Results contain only HRESULTs, not
game data.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
from tempfile import TemporaryDirectory

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from build import Toolchain
from project_tuxbox.host import proton_installation
from project_tuxbox.inspect import digest


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--original-xg', required=True, type=Path)
    parser.add_argument('--provider', required=True, type=Path)
    parser.add_argument('--xmem', required=True, type=Path)
    parser.add_argument('--proton', type=Path)
    parser.add_argument('--prefix', required=True, type=Path)
    parser.add_argument('--output', required=True, type=Path)
    args = parser.parse_args()
    proton = proton_installation(args.proton)
    wine = proton / 'files/bin/wine'
    if not wine.is_file():
        parser.error('a valid Proton installation is required')
    env = os.environ.copy()
    env.update(WINEPREFIX=str(args.prefix.resolve()), WINEDEBUG='-all', WINEDLLOVERRIDES='xg_subject,xmem=n')
    base = ROOT / 'build/xg-depth-swizzle-control'
    base.mkdir(parents=True, exist_ok=True)
    with TemporaryDirectory(prefix='differential-', dir=base) as tmp:
        work = Path(tmp)
        source = ROOT / 'runtime/xg/depth_swizzle_control.cpp'
        toolchain = Toolchain()
        obj = toolchain.compile(source, work, '/W4', '/WX')
        exe = work / 'depth-swizzle-control.exe'
        toolchain.link(exe, obj, 'kernel32.lib')
        results, outputs = {}, []
        for name, dll in [('original', args.original_xg), ('tuxbox', args.provider)]:
            trial = work / name
            trial.mkdir()
            guest = trial / exe.name
            shutil.copy2(exe, guest)
            shutil.copy2(dll, trial / 'xg_subject.dll')
            shutil.copy2(args.xmem, trial / 'xmem.dll')
            run = subprocess.run([str(wine), str(guest)], cwd=trial, env=env, timeout=60,
                                 stdout=subprocess.PIPE, stderr=subprocess.STDOUT, check=False)
            out = run.stdout.decode('utf-8', errors='replace').strip()
            if run.returncode or 'xg-depth-swizzle PASS 6125 cases' not in out:
                raise RuntimeError(f'{name} control failed ({run.returncode}): {out}')
            data = (trial / 'depth-swizzle-results.bin').read_bytes()
            if len(data) != 6125 * 4:
                raise ValueError(f'{name} emitted an unexpected result size')
            outputs.append(data)
            results[name] = {'dll_sha256': digest(dll), 'guest_receipt': out.splitlines()[-1],
                             'result_sha256': hashlib.sha256(data).hexdigest()}
        if outputs[0] != outputs[1]:
            offset = next(i for i, (a, b) in enumerate(zip(*outputs)) if a != b)
            raise ValueError(f'XG depth-swizzle results differ at byte {offset}, case {offset // 4}')
        receipt = {
            'scope': 'XGValidateDepthStencilSwizzleMode ordinal 21. HRESULTs compared with supplied Xbox XG DLL for representative formats, sample counts, all modes 0 through 31 and integer boundary modes.',
            'host': (proton / 'version').read_text().strip(),
            'guest_sha256': digest(exe), 'guest_source_sha256': digest(source),
            'xmem_sha256': digest(args.xmem),
            'cases': 6125,
            'checks': ['25 representative formats including unknown and boundary values', 'seven sample counts including invalid extremes', '32 contiguous swizzle modes and three integer boundary modes'],
            'runs': results,
        }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(receipt, indent=2) + '\n')
    print('xg-depth-swizzle differential PASS: 6125 cases match')


if __name__ == '__main__':
    main()
