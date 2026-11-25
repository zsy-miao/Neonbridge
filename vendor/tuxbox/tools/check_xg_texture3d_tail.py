#!/usr/bin/env python3
"""Compare Xbox XG 3D texture exports with the TuxBox provider.

The same Windows guest runs with each DLL and compares every output byte for
13,760 supported-subset layouts and 5,191,200 sampled element addresses. No game
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
    base = ROOT / 'build/xg-texture3d-control'
    base.mkdir(parents=True, exist_ok=True)
    with TemporaryDirectory(prefix='differential-', dir=base) as tmp:
        work = Path(tmp)
        source = ROOT / 'runtime/xg/texture3d_tail_control.cpp'
        toolchain = Toolchain()
        obj = toolchain.compile(source, work, '/W4', '/WX')
        exe = work / 'texture3d-control.exe'
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
            if run.returncode or 'xg-3d-tail PASS 13760 layouts 5191200 element addresses' not in out:
                raise RuntimeError(f'{name} control failed ({run.returncode}): {out}')
            data = (trial / 'texture3d-tail-results.bin').read_bytes()
            if len(data) != 13760 * 0x1b40 + 5191200 * 12:
                raise ValueError(f'{name} emitted an unexpected number of result bytes')
            outputs.append(data)
            results[name] = {'dll_sha256': digest(dll), 'guest_receipt': out.splitlines()[-1],
                             'result_sha256': hashlib.sha256(data).hexdigest()}
        if outputs[0] != outputs[1]:
            offset = next(i for i, (a, b) in enumerate(zip(*outputs)) if a != b)
            raise ValueError(f'XG 3D mip results differ at byte {offset}')
        check = subprocess.run([str(wine), str(work / 'tuxbox' / exe.name), 'unsupported'],
                               cwd=work / 'tuxbox', env=env, timeout=60,
                               stdout=subprocess.PIPE, stderr=subprocess.STDOUT, check=False)
        if check.returncode or b'unsupported descriptor rejected' not in check.stdout:
            raise RuntimeError(f'unsupported-subset control failed: {check.stdout!r}')
        receipt = {
            'scope': 'XGComputeTexture3DLayout (ordinal 6), XGCreateTexture3DComputer (ordinal 10) and generic XGCreateTextureComputer (ordinal 11) for a bounded 3D subset. Identical guest and XMem dependency; complete layouts and sampled element addresses compared against the supplied Xbox XG DLL.',
            'subset': '57 uncompressed color formats; widths 8, 16, 32, 64 or 128 and heights 1 to 4; depth 1, 8, 17, 128 or 2,048; two to four mips with one or two packed tail levels; shader-resource bind; 4 KiB swizzle with default or immutable usage; no CPU access flags, misc flags or explicit pitch. Depth-only mip chains, block-compressed formats and other combinations are unsupported.',
            'host': (proton / 'version').read_text().strip(),
            'guest_sha256': digest(exe), 'guest_source_sha256': digest(source),
            'xmem_sha256': digest(args.xmem),
            'layouts': 13760, 'sampled_element_addresses': 5191200,
            'checks': ['all three exports and complete matching layouts', 'sampled mip and depth-slice addresses including the first out-of-range slice',
                       'null arguments and guards', 'unverified usage rejected by provider'],
            'runs': results,
        }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(receipt, indent=2) + '\n')
    print('xg-3d-tail differential PASS: 13760 layouts and 5191200 addresses match')


if __name__ == '__main__':
    main()
