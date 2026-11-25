#!/usr/bin/env python3
"""Compare XGComputeOptimalSwizzleMode with the supplied Xbox library.

The same guest calls the Xbox DLL and TuxBox provider with 70,200 combinations
of resource dimensions, formats and flags. Results contain only selected mode
values, not game data.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
from tempfile import TemporaryDirectory
import pefile

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from build import Toolchain
from project_tuxbox.host import proton_installation
from project_tuxbox.inspect import digest


def verify_xgs_callsite(path):
    """Find a supplied XGS call that places all four stack arguments."""
    pe = pefile.PE(str(path), fast_load=False)
    addresses = [entry.address for table in pe.DIRECTORY_ENTRY_IMPORT
                 if table.dll.lower() == b'xg_xs.dll' for entry in table.imports
                 if entry.name == b'XGComputeOptimalSwizzleMode']
    if len(addresses) != 1:
        raise ValueError(f'{path}: missing or ambiguous XG swizzle import')
    output = subprocess.check_output(['llvm-objdump', '-d', '--no-show-raw-insn', str(path)], text=True)
    lines = output.splitlines()
    address = f'0x{addresses[0]:x}'
    matches = []
    for index, line in enumerate(lines):
        if not re.search(r'\bcallq?\s+\*.*#\s+' + re.escape(address) + r'\b', line):
            continue
        before = '\n'.join(lines[max(0, index-12):index])
        if all(re.search(r'\bmovl\s+%eax,\s*0x' + slot + r'\(%rsp\)', before)
               for slot in ('20', '28', '30', '38')):
            match = re.match(r'^\s*([0-9a-f]+):', line)
            if match:
                matches.append(int(match.group(1), 16) - pe.OPTIONAL_HEADER.ImageBase)
    if not matches:
        raise ValueError(f'{path}: no verified eight-argument XG swizzle call')
    return {'binary_sha256': digest(path), 'import_address': address,
            'call_rvas': [hex(rva) for rva in matches],
            'eighth_argument': 'Caller writes the fourth stack argument at rsp+0x38 before call.'}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--original-xg', required=True, type=Path)
    parser.add_argument('--provider', required=True, type=Path)
    parser.add_argument('--xmem', required=True, type=Path)
    parser.add_argument('--xgs', required=True, type=Path, action='append',
                        help='supplied XGS DLL with an XG swizzle import; may be repeated')
    parser.add_argument('--proton', type=Path)
    parser.add_argument('--prefix', required=True, type=Path)
    parser.add_argument('--output', required=True, type=Path)
    args = parser.parse_args()
    callsites = {path.name: verify_xgs_callsite(path) for path in args.xgs}
    proton = proton_installation(args.proton)
    wine = proton / 'files/bin/wine'
    if not wine.is_file():
        parser.error('a valid Proton installation is required')
    env = os.environ.copy()
    env.update(WINEPREFIX=str(args.prefix.resolve()), WINEDEBUG='-all', WINEDLLOVERRIDES='xg_subject,xmem=n')
    base = ROOT / 'build/xg-optimal-swizzle-control'
    base.mkdir(parents=True, exist_ok=True)
    with TemporaryDirectory(prefix='differential-', dir=base) as tmp:
        work = Path(tmp)
        source = ROOT / 'runtime/xg/optimal_swizzle_control.cpp'
        toolchain = Toolchain()
        obj = toolchain.compile(source, work, '/W4', '/WX')
        exe = work / 'optimal-swizzle-control.exe'
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
            if run.returncode or 'xg-optimal-swizzle PASS 70200 cases' not in out:
                raise RuntimeError(f'{name} control failed ({run.returncode}): {out}')
            data = (trial / 'optimal-swizzle-results.bin').read_bytes()
            if len(data) != 70200 * 4:
                raise ValueError(f'{name} emitted an unexpected result size')
            outputs.append(data)
            results[name] = {'dll_sha256': digest(dll), 'guest_receipt': out.splitlines()[-1],
                             'result_sha256': hashlib.sha256(data).hexdigest()}
        if outputs[0] != outputs[1]:
            offset = next(i for i, (a, b) in enumerate(zip(*outputs)) if a != b)
            raise ValueError(f'XG optimal-swizzle results differ at byte {offset}, case {offset // 4}')
        receipt = {
            'scope': 'XGComputeOptimalSwizzleMode ordinal 3. Returned mode values compared with supplied Xbox XG DLL for dimensions, formats, resource and bind flags, sizes and sample-count boundaries. The control supplies the eighth flags argument explicitly.',
            'host': (proton / 'version').read_text().strip(),
            'guest_sha256': digest(exe), 'guest_source_sha256': digest(source),
            'xmem_sha256': digest(args.xmem),
            'cases': 70200,
            'xgs_callsite_checks': callsites,
            'checks': ['six resource dimensions', 'formats 0–127 and two integer boundaries', 'six bind flag sets and five resource flag sets', 'zero, ordinary and maximum size/sample parameters'],
            'runs': results,
        }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(receipt, indent=2) + '\n')
    print('xg-optimal-swizzle differential PASS: 70200 cases match')


if __name__ == '__main__':
    main()
