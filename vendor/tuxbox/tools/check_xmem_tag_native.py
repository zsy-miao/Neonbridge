#!/usr/bin/env python3
"""Check the supplied x64 XMemMakeTag code against the guest fingerprint.

Runs only the isolated, fingerprinted, call-free XMemMakeTag routine from the
user-supplied package; it does not load the console DLL or save its bytes.
"""
import argparse
import hashlib
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
REFERENCE = "adc182acae5340b0618a3d57a239ed2e8deb305dbefc49439cb8a2d06c7ce812"
EXPECTED = "zero=0000000000000000 sample=69a69a0000000000 count=5120 hash=d4bf30c57509b176"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("dll", type=Path, help="locally supplied xmem.dll")
    args = parser.parse_args()
    if hashlib.sha256(args.dll.read_bytes()).hexdigest() != REFERENCE:
        raise SystemExit("Unsupported xmem.dll fingerprint; native control not run")
    with tempfile.TemporaryDirectory(prefix="tuxbox-xmem-tag-") as tmp:
        guest = Path(tmp) / "xmem-tag-native"
        subprocess.run(["cc", "-std=gnu11", "-O2", ROOT / "tools/reference/xmem-tag/native.c", "-o", guest], check=True)
        result = subprocess.run([guest, args.dll.resolve()], capture_output=True, text=True, check=True).stdout.strip()
    if result != EXPECTED:
        raise SystemExit(f"Reference tag result mismatch: {result}")
    print(result)


if __name__ == "__main__":
    main()
