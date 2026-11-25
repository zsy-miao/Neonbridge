#!/usr/bin/env python3
"""Compare Xbox XG 2D texture exports with the TuxBox provider.

The same Windows guest runs with each DLL and compares every output byte for
2,964 supported-subset layouts and 34,632 sampled element addresses. No game
bytes are distributed. Supply the original XG DLL and a TuxBox XMem DLL.
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
    env.update(WINEPREFIX=str(args.prefix.resolve()), WINEDEBUG='-all',
               WINEDLLOVERRIDES='xg_subject,xmem=n')
    base = ROOT / 'build/xg-texture2d-control'
    base.mkdir(parents=True, exist_ok=True)
    with TemporaryDirectory(prefix='differential-', dir=base) as tmp:
        work = Path(tmp)
        source = ROOT / 'runtime/xg/texture_uav2d_control.cpp'
        toolchain = Toolchain()
        obj = toolchain.compile(source, work, '/W4', '/WX')
        exe = work / 'texture2d-control.exe'
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
            if run.returncode or 'xg-uav2d PASS 2964 layouts 34632 element addresses' not in out:
                raise RuntimeError(f'{name} control failed ({run.returncode}): {out}')
            data = (trial / 'texture-uav2d-results.bin').read_bytes()
            if len(data) != 2964 * 0x1b40 + 34632 * 12:
                raise ValueError(f'{name} emitted an unexpected number of result bytes')
            outputs.append(data)
            results[name] = {'dll_sha256': digest(dll), 'guest_receipt': out.splitlines()[-1],
                             'result_sha256': hashlib.sha256(data).hexdigest()}
        if outputs[0] != outputs[1]:
            offset = next(i for i, (a, b) in enumerate(zip(*outputs)) if a != b)
            raise ValueError(f'XG UAV 2D results differ at byte {offset}')
        check = subprocess.run([str(wine), str(work / 'tuxbox' / exe.name), 'unsupported'],
                               cwd=work / 'tuxbox', env=env, timeout=60,
                               stdout=subprocess.PIPE, stderr=subprocess.STDOUT, check=False)
        if check.returncode or b'unsupported descriptor rejected' not in check.stdout:
            raise RuntimeError(f'unsupported-subset control failed: {check.stdout!r}')
        receipt = {
            'scope': 'XGCreateTextureComputer (ordinal 11) with default layout 0 for a bounded subset. Identical guest and XMem dependency; complete layouts and sampled element addresses compared against the supplied Xbox XG DLL.',
            'subset': '78 color/block formats; ten square and rectangular shapes including Frosty 38x1 RGBA32, 1x1 and one-pixel edges; one or three mips and slices; one sample; default layout 0; flags 4, zero alignment and sample quality. Other combinations are unsupported.',
            'host': (proton / 'version').read_text().strip(),
            'guest_sha256': digest(exe), 'guest_source_sha256': digest(source),
            'xmem_sha256': digest(args.xmem),
            'layouts': 2964, 'sampled_element_addresses': 34632,
            'checks': ['complete layouts', 'sampled mip and array-slice addresses',
                       'output guards', 'unverified flags rejected by provider'],
            'runs': results,
        }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(receipt, indent=2) + '\n')
    print('xg-uav2d differential PASS: 2964 layouts and 34632 addresses match')


if __name__ == '__main__':
    main()
