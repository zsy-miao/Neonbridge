#!/usr/bin/env python3
"""Build a temporary XG tracing pair into a new empty directory; do not launch a game."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import sys

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))
from build import Toolchain


def sha(path):
    return hashlib.file_digest(path.open('rb'), 'sha256').hexdigest()


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--original-xg', type=Path, required=True)
    p.add_argument('--unique', action='store_true', help='record each distinct descriptor and result once')
    p.add_argument('--output', type=Path, required=True)
    args = p.parse_args()
    original = args.original_xg.resolve(strict=True)
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=False)
    here = Path(__file__).resolve().parent
    toolchain = Toolchain()
    flags = ['/DTRACE_UNIQUE=1'] if args.unique else []
    obj = toolchain.compile(here / 'proxy.c', out, '/W4', '/WX', *flags)
    toolchain.link(out / 'xg_xs.dll', obj, 'kernel32.lib', exports=here / 'xg_xs.def')
    obj = toolchain.compile(here / 'control.c', out, '/W4', '/WX')
    toolchain.link(out / 'xg-proxy-control.exe', obj, 'kernel32.lib')
    shutil.copyfile(original, out / 'xg_reference.dll')
    (out / 'provenance.json').write_text(json.dumps({
        'purpose': 'temporary local XG descriptor tracing; genuine reference binary is not redistributable',
        'reference_source': str(original), 'reference_sha256': sha(original),
        'proxy_sha256': sha(out / 'xg_xs.dll'), 'control_sha256': sha(out / 'xg-proxy-control.exe'),
        'log_limit': 16384, 'unique_descriptors': args.unique, 'instrumented_ordinal': 11,
    }, indent=2) + '\n')
    print('Prepared isolated reference proxy; no title was launched.')


if __name__ == '__main__':
    main()
