#!/usr/bin/env python3
"""Compare the supplied Xbox XG buffer-layout export with the TuxBox provider.

Runs the same guest and XMem dependency against each XG DLL. The guest checks
null arguments, error-output preservation and guards; this script compares all
layout bytes and HRESULTs for 967 descriptors. No game bytes are included in the
receipt. Supply the original XG DLL and a working TuxBox XMem provider locally.
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
    parser.add_argument('--prefix', type=Path)
    parser.add_argument('--output', required=True, type=Path)
    args = parser.parse_args()
    proton = proton_installation(args.proton)
    wine = proton / 'files/bin/wine'
    if not wine.is_file() or not args.prefix:
        parser.error('a Proton installation and --prefix are required')
    environment = os.environ.copy()
    environment.update(WINEPREFIX=str(args.prefix.resolve()), WINEDEBUG='-all',
                       WINEDLLOVERRIDES='xg_subject,xmem=n')
    base = ROOT / 'build/xg-buffer-control'
    base.mkdir(parents=True, exist_ok=True)
    with TemporaryDirectory(prefix='differential-', dir=base) as temp:
        work = Path(temp)
        source = ROOT / 'runtime/xg/buffer_control.cpp'
        toolchain = Toolchain()
        obj = toolchain.compile(source, work, '/W4', '/WX')
        exe = work / 'buffer-control.exe'
        toolchain.link(exe, obj, 'kernel32.lib')
        guest_hash = digest(exe)
        results = {}
        outputs = []
        for name, dll in [('original', args.original_xg), ('tuxbox', args.provider)]:
            trial = work / name
            trial.mkdir()
            guest = trial / exe.name
            shutil.copy2(exe, guest)
            shutil.copy2(dll, trial / 'xg_subject.dll')
            shutil.copy2(args.xmem, trial / 'xmem.dll')
            run = subprocess.run([str(wine), str(guest)], cwd=trial, env=environment, timeout=60,
                                 stdout=subprocess.PIPE, stderr=subprocess.STDOUT, check=False)
            text = run.stdout.decode('utf-8', errors='replace').strip()
            if run.returncode or 'xg-buffer PASS 967 descriptors' not in text:
                raise RuntimeError(f'{name} control failed ({run.returncode}): {text}')
            data = (trial / 'buffer-results.bin').read_bytes()
            if len(data) != 967 * 0x1b84:
                raise ValueError(f'{name} emitted an unexpected number of result bytes')
            outputs.append(data)
            results[name] = {'dll_sha256': digest(dll), 'guest_receipt': text.splitlines()[-1],
                             'result_sha256': hashlib.sha256(data).hexdigest()}
        if outputs[0] != outputs[1]:
            offset = next(i for i, (a, b) in enumerate(zip(*outputs)) if a != b)
            raise ValueError(f'XG buffer results differ at byte {offset}, descriptor {offset // 0x1b84}')
        receipt = {
            'scope': 'XGComputeBufferLayout export, ordinal 1. Complete output bytes and HRESULTs compared against the supplied Xbox XG DLL; the same XMem provider and guest are used for both runs.',
            'host': (proton / 'version').read_text().strip(),
            'guest_sha256': guest_hash,
            'guest_source_sha256': digest(source),
            'xmem_sha256': digest(args.xmem),
            'descriptors': 967,
            'result_bytes_per_descriptor': 0x1b84,
            'checks': ['zero, boundary and maximum sizes', 'each input flag bit',
                       'deterministic combinations and reserved fields', 'null pointers',
                       'unchanged output on invalid arguments', 'full layout and surrounding guards'],
            'runs': results,
        }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(receipt, indent=2) + '\n')
    print('xg-buffer differential PASS: 967 complete layouts and HRESULTs match')


if __name__ == '__main__':
    main()
