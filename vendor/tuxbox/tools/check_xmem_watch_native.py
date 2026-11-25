#!/usr/bin/env python3
"""Verify the hash-checked console XMemGetWriteWatch dispatch in isolation.

Executes the supplied DLL's mapped routine with a controlled native callback
at its imported GetWriteWatch slot. No console bytes are saved in the repo.
"""
import argparse
import hashlib
from pathlib import Path
import subprocess
import tempfile
import pefile

ROOT = Path(__file__).resolve().parents[1]
REFERENCE = "adc182acae5340b0618a3d57a239ed2e8deb305dbefc49439cb8a2d06c7ce812"
EXPECTED = "xmem-native-watch PASS forwarded=6 paused=1003 output=untouched option=2 resume=3"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("dll", type=Path, help="locally supplied xmem.dll")
    args = parser.parse_args()
    data = args.dll.read_bytes()
    if hashlib.sha256(data).hexdigest() != REFERENCE:
        raise SystemExit("Unsupported xmem.dll fingerprint; native control not run")
    image = pefile.PE(data=data).get_memory_mapped_image()
    with tempfile.TemporaryDirectory(prefix="tuxbox-xmem-watch-") as tmp:
        temp = Path(tmp)
        (temp / "image").write_bytes(image)
        exe = temp / "native-control"
        subprocess.run(["cc", "-std=gnu11", "-O2", ROOT / "tools/reference/xmem-tag/watch_native.c", "-o", exe], check=True)
        result = subprocess.run([exe, temp / "image"], capture_output=True, text=True, check=True).stdout.strip()
    if result != EXPECTED:
        raise SystemExit(f"Reference watch result mismatch: {result}")
    print(result)


if __name__ == "__main__":
    main()
