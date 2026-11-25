"""kernelx.dll : le noyau de la Xbox redirigé vers kernel32/kernelbase du PC."""
import os
import sys
from pathlib import Path

import pytest

from pe_builder import Asm, build_pe

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tools"))

import kernelx_exports  # noqa: E402


def test_kernel32_is_preferred_and_odd_names_are_skipped():
    table = kernelx_exports.forwarders([("kernel32", ["CreateFileW", "Shared"]),
                                        ("kernelbase", ["Shared", "PathCchCombine", "not-an-identifier"])])
    assert table == {"CreateFileW": "kernel32", "PathCchCombine": "kernelbase", "Shared": "kernel32"}


def test_include_lists_linker_forwarders():
    text = kernelx_exports.to_include({"CreateFileW": "kernel32"}, "test")
    assert '#pragma comment(linker, "/export:CreateFileW=kernel32.CreateFileW")' in text


@pytest.mark.skipif(sys.platform != "win32", reason="exécution native sous Windows")
def test_console_kernel_import_reaches_windows(tmp_path, runtime_dir):
    from emu.native import NativeImage

    def code(iat):
        return Asm().frame().call_iat(iat[("kernelx.dll", "GetCurrentProcessId")]).unframe().ret().build()

    exe = build_pe(tmp_path / "console.exe", code, imports=[("kernelx.dll", "GetCurrentProcessId")], dll=False)
    image = NativeImage(exe)
    try:
        assert image.call_entry() & 0xFFFF_FFFF == os.getpid()
    finally:
        image.close()
