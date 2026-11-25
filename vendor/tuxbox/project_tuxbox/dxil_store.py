"""Reuse validated DXIL when a compiler update leaves a shader's HLSL unchanged.

The runtime cache is keyed by the translator binary, so each translator change
starts an empty cache. Most shaders still translate to the same HLSL, and DXC
is nearly all of the compile time. This store keys DXIL by what DXC's output
depends on: the HLSL, the target profile, and the DXC and validator in use.
"""
from collections import Counter
import hashlib
import os
from pathlib import Path
import re
import shutil
from tempfile import mkstemp

from .inspect import digest
from .shader_prep import has_dxil_container


# Bump when a policy change can make cached DXIL unacceptable under validation.
# Output-equivalent optimisation and fallback variants share the policy value.
POLICY = 1
IDENTITY_FILE = "dxil-identity"
LIBRARIES = ("libdxcompiler*", "libdxil*", "dxcompiler.dll", "dxil.dll")
reused = Counter()


def write_identity(cache, version, dxc, dxv):
    """Record which DXC toolchain fills this runtime cache. This turns reuse on."""
    dxc, dxv = Path(dxc), Path(dxv)
    libraries = sorted({path for directory in (dxc.parent, dxc.parent.parent / "lib")
                        for pattern in LIBRARIES for path in directory.glob(pattern) if path.is_file()})
    identity = hashlib.sha256(f"{POLICY}\n".encode() + version)
    for tool in (dxc, dxv, *libraries):
        identity.update(bytes.fromhex(digest(tool)))
    _publish(cache / IDENTITY_FILE, identity.hexdigest().encode("ascii"))


def _publish(path, data):
    """Replace `path` atomically with `data`."""
    handle, pending = mkstemp(dir=path.parent, prefix=".pending-")
    try:
        with os.fdopen(handle, "wb") as stream:
            stream.write(data)
        os.replace(pending, path)
    finally:
        Path(pending).unlink(missing_ok=True)


class DxilStore:
    def __init__(self, root):
        self.root = root

    @classmethod
    def for_cache(cls, cache):
        """The store behind `cache`, or None when reuse is not enabled for it."""
        try:
            identity = (cache / IDENTITY_FILE).read_text(encoding="ascii")
            if not re.fullmatch(r"[a-f0-9]{64}", identity):
                return None
            # <cache root>/runtime/<translator fingerprint> -> <cache root>/dxil/<toolchain>
            root = cache.parent.parent / "dxil" / identity
            root.mkdir(parents=True, exist_ok=True)
        except (OSError, UnicodeError):
            return None
        return cls(root)

    @staticmethod
    def key(hlsl, target):
        return hashlib.sha256(f"{target}\n".encode("ascii") + hlsl).hexdigest()

    def fetch(self, key, output):
        """Copy the stored DXIL to `output` if its bytes still match their stamp."""
        try:
            expected = (self.root / f"{key}.sha256").read_text(encoding="ascii")
            shutil.copyfile(self.root / f"{key}.cso", output)
            if digest(output) == expected and has_dxil_container(output.read_bytes()):
                reused["dxil"] += 1
                return True
        except (OSError, UnicodeError):
            pass
        output.unlink(missing_ok=True)
        return False

    def save(self, key, output):
        """Keep validated DXIL. A failure only costs a later recompile."""
        try:
            data = output.read_bytes()
            _publish(self.root / f"{key}.cso", data)
            _publish(self.root / f"{key}.sha256", hashlib.sha256(data).hexdigest().encode("ascii"))
        except OSError:
            pass

    def is_slow(self, request, target):
        """Whether this request's optimised compile has timed out before."""
        return (self.root / f"{request}.{target}.slow").is_file()

    def mark_slow(self, request, target):
        try:
            (self.root / f"{request}.{target}.slow").touch()
        except OSError:
            pass
