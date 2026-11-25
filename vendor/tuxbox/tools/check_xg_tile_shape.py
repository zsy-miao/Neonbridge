#!/usr/bin/env python3
"""Compare XGComputeTileShape with the supplied Xbox library.

The same guest calls the Xbox DLL and TuxBox provider with 18,290 cases,
including supported color formats, buffer and texture dimensions, sample
counts, invalid input, null outputs and aliased outputs. Results contain only
HRESULTs and generated shape/guard bytes, not game data.
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
    base = ROOT / 'build/xg-tile-shape-control'
    base.mkdir(parents=True, exist_ok=True)
    with TemporaryDirectory(prefix='differential-', dir=base) as tmp:
        work = Path(tmp)
        source = ROOT / 'runtime/xg/tile_shape_control.cpp'
        toolchain = Toolchain()
        obj = toolchain.compile(source, work, '/W4', '/WX')
        exe = work / 'tile-control.exe'
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
            if run.returncode or 'xg-tile PASS 18290 cases' not in out:
                raise RuntimeError(f'{name} control failed ({run.returncode}): {out}')
            data = (trial / 'tile-results.bin').read_bytes()
            if len(data) != 18290 * 92:
                raise ValueError(f'{name} emitted an unexpected result size')
            outputs.append(data)
            results[name] = {'dll_sha256': digest(dll), 'guest_receipt': out.splitlines()[-1],
                             'result_sha256': hashlib.sha256(data).hexdigest()}
        if outputs[0] != outputs[1]:
            offset = next(i for i, (a, b) in enumerate(zip(*outputs)) if a != b)
            raise ValueError(f'XG tile results differ at byte {offset}, case {offset // 92}')
        receipt = {
            'scope': 'XGComputeTileShape ordinal 7 for buffers and 120 Xbox formats in 2D. HRESULT, output and guards compared against supplied Xbox XG DLL. 1D/3D texture tile shapes remain unsupported.',
            'host': (proton / 'version').read_text().strip(),
            'guest_sha256': digest(exe), 'guest_source_sha256': digest(source),
            'xmem_sha256': digest(args.xmem),
            'cases': 18290,
            'checks': ['120 color, depth, video, packed and block-compressed formats', 'unknown-format explicit element sizes',
                       '1, 2, 4 and 8 samples plus invalid sample counts',
                       'buffer, 1D, 2D and 3D dimensions, including all 120 formats for 1D/3D', 'null and aliased output pointers',
                       'output guards and no writes on invalid arguments'],
            'runs': results,
        }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(receipt, indent=2) + '\n')
    print('xg-tile differential PASS: 18290 cases match')


if __name__ == '__main__':
    main()
