"""Mode natif avec les DLL console de TomBox par défaut (tools/build_tombox.py, runtime/build/tombox)."""
import os
import subprocess
import sys

import pytest

from emu import tombox_paths
from pe_builder import Asm, build_pe
from test_xcrdapi import IID_FACTORY_X

pytestmark = pytest.mark.skipif(sys.platform != "win32", reason="exécution native sous Windows")

IID_ID3D12_DEVICE = bytes.fromhex("f1199818b61d574bbe541821339b85f7")   # ID3D12Device, little-endian
IID_OFFSET, PARAMS_OFFSET, OUT_OFFSET = 0x200, 0x210, 0x220             # données après le code
PAGE_READWRITE, MEM_RESERVE_PHYSICAL_LARGE = 0x04, 0x20402000           # XMemVirtualAlloc (W1)


def tombox_built():
    return all((tombox_paths.TOMBOX_INSTALL / name).is_file() for name in tombox_paths.TOMBOX_PREFERRED)


@pytest.fixture
def native_tombox(runtime_dir):
    """(anyxboxx.exe, environnement sans ANYXBOXX_TOMBOX_DIR) : le dossier TomBox par défaut ; absent : ignoré."""
    if not tombox_built():
        if os.environ.get("ANYXBOXX_REQUIRE_RUNTIME") == "1":
            pytest.fail(f"DLL de TomBox absentes de {tombox_paths.TOMBOX_INSTALL} : {tombox_paths.BUILD_COMMAND}")
        pytest.skip(f"DLL de TomBox non compilées ({tombox_paths.BUILD_COMMAND})")
    environment = {key: value for key, value in os.environ.items() if key != tombox_paths.TOMBOX_ENV}
    return runtime_dir / "anyxboxx.exe", environment   # sans variable : le lanceur doit trouver ..\tombox seul


def _data(asm):
    """Pad jusqu'aux données (IID, paramètres, pointeur de sortie) placées après le code."""
    return (asm.at(IID_OFFSET).raw(IID_ID3D12_DEVICE).raw(bytes(16)).raw(bytes(8)))


def test_d3d12_xbox_create_device_returns_a_device(tmp_path, native_tombox):
    """D3D12XboxCreateDevice(NULL, params, IID_ID3D12Device, &device) rend un device non nul."""
    launcher, environment = native_tombox
    def code(iat):
        asm = (Asm().frame().zero_rcx().lea_rdx(PARAMS_OFFSET).lea_r8(IID_OFFSET).lea_r9(OUT_OFFSET)
               .call_iat(iat[("d3d12_xs.dll", "D3D12XboxCreateDevice")])
               .mov_rax_from(OUT_OFFSET).nonzero_rax_to_eax().unframe().ret())
        return _data(asm).build()

    exe = build_pe(tmp_path / "device.exe", code, imports=[("d3d12_xs.dll", "D3D12XboxCreateDevice")], dll=False)
    result = subprocess.run([str(launcher), str(exe)], capture_output=True, text=True,
                            timeout=120, env=environment)
    assert result.returncode == 1, result.stderr


def test_tombox_xmem_virtual_alloc_never_returns_null_for_physical_reserve(tmp_path, native_tombox):
    """W1 : XMemVirtualAlloc(NULL, 64 Ko, 0x20402000, attributs, PAGE_READWRITE) rend une adresse."""
    launcher, environment = native_tombox
    def code(iat):
        asm = (Asm().frame().zero_rcx().mov_edx(0x10000).mov_r8d(MEM_RESERVE_PHYSICAL_LARGE).mov_r9d(0x3)
               .stack_dword(0x20, PAGE_READWRITE)
               .call_iat(iat[("xmem.dll", "XMemVirtualAlloc")])
               .nonzero_rax_to_eax().unframe().ret())
        return _data(asm).build()

    exe = build_pe(tmp_path / "xmem.exe", code, imports=[("xmem.dll", "XMemVirtualAlloc")], dll=False)
    result = subprocess.run([str(launcher), str(exe)], capture_output=True, text=True,
                            timeout=120, env=environment)
    assert result.returncode == 1, result.stderr


def test_dstorage_factory_is_written_by_tombox_xcrdapi(tmp_path, native_tombox):
    """W5 : DStorageGetFactory écrit une fabrique dans *ppv (notre XCRDAPI minimal ne le faisait pas)."""
    launcher, environment = native_tombox

    def code(iat):
        asm = (Asm().frame().lea_rcx(IID_OFFSET).lea_rdx(OUT_OFFSET)
               .call_iat(iat[("XCRDAPI.dll", "DStorageGetFactory")])
               .mov_rax_from(OUT_OFFSET).nonzero_rax_to_eax().unframe().ret())
        return asm.at(IID_OFFSET).raw(IID_FACTORY_X).at(OUT_OFFSET).raw(bytes(8)).build()

    exe = build_pe(tmp_path / "dstorage.exe", code, imports=[("XCRDAPI.dll", "DStorageGetFactory")], dll=False)
    result = subprocess.run([str(launcher), str(exe)], capture_output=True, text=True, timeout=120, env=environment)
    assert result.returncode == 1, result.stderr
