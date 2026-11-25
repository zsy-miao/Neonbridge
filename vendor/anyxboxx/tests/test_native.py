"""Exécution native façon Wine : le code tourne sur le vrai processeur.

Les PE de test sont fabriqués par pe_builder : leur contenu est connu octet par octet.
"""
import os
import sys

import pytest

from pe_builder import Asm, build_pe

pytestmark = pytest.mark.skipif(sys.platform != "win32", reason="exécution native sous Windows")

E_NOINTERFACE = 0x80004002
WIDE_XGR = 0x200     # L"xgameruntime.dll"
NAME_QUERY = 0x240   # "QueryApiImpl"
DLL_MAIN = 0x80     # DllMain des DLL de test : renvoie TRUE


def run(path):
    from emu.native import NativeImage

    image = NativeImage(path)
    try:
        return image.call_entry() & 0xFFFF_FFFF, image.base
    finally:
        image.close()


def test_entry_point_runs_on_real_cpu(tmp_path, runtime_dir):
    exe = build_pe(tmp_path / "answer.exe", lambda iat: Asm().mov_eax(42).ret().build(), dll=False)
    assert run(exe)[0] == 42


def test_import_is_bound_to_real_windows_function(tmp_path, runtime_dir):
    def code(iat):
        return Asm().frame().call_iat(iat[("kernel32.dll", "GetCurrentProcessId")]).unframe().ret().build()

    exe = build_pe(tmp_path / "pid.exe", code, imports=[("kernel32.dll", "GetCurrentProcessId")], dll=False)
    assert run(exe)[0] == os.getpid()


def test_module_handle_of_game_is_its_own_base(tmp_path, runtime_dir):
    def code(iat):
        return (Asm().frame().zero_rcx()
                .call_iat(iat[("kernel32.dll", "GetModuleHandleW")]).unframe().ret().build())

    exe = build_pe(tmp_path / "self.exe", code, imports=[("kernel32.dll", "GetModuleHandleW")], dll=False)
    code_value, base = run(exe)
    assert code_value == base & 0xFFFF_FFFF


def test_xgameruntime_is_served_by_anyxboxx(tmp_path, runtime_dir):
    """LoadLibraryExW("xgameruntime.dll") puis GetProcAddress("QueryApiImpl") puis appel."""
    def code(iat):
        asm = (Asm().frame()
               .lea_rcx(WIDE_XGR).zero_rdx().zero_r8()
               .call_iat(iat[("kernel32.dll", "LoadLibraryExW")])
               .mov_rcx_rax().lea_rdx(NAME_QUERY)
               .call_iat(iat[("kernel32.dll", "GetProcAddress")])
               .zero_rcx().zero_rdx().zero_r8().call_rax()
               .unframe().ret())
        asm.at(WIDE_XGR).raw("xgameruntime.dll".encode("utf-16-le") + b"\0\0")
        asm.at(NAME_QUERY).raw(b"QueryApiImpl\0")
        return asm.build()

    imports = [("kernel32.dll", "LoadLibraryExW"), ("kernel32.dll", "GetProcAddress")]
    exe = build_pe(tmp_path / "gdk.exe", code, imports=imports, dll=False)
    assert run(exe)[0] == E_NOINTERFACE


def test_relocated_when_preferred_base_is_taken(tmp_path, runtime_dir):
    from emu.native import NativeImage

    first = build_pe(tmp_path / "a.exe", lambda iat: Asm().mov_eax(1).ret().build(), dll=False)
    second = build_pe(tmp_path / "b.exe", lambda iat: Asm().mov_eax(2).ret().at(0x80).build(),
                      dll=False, pointers={0x80: 0})
    a = NativeImage(first)
    try:
        b = NativeImage(second)
        try:
            assert b.base != a.base and b.call_entry() == 2
        finally:
            b.close()
    finally:
        a.close()


def test_game_sees_its_own_command_line(tmp_path):
    """Sous-processus : la ligne de commande du processus est celle du jeu, pas de python."""
    import subprocess

    script = ("import ctypes, sys; sys.path.insert(0, 'PrototypeScarlett');"
              "from emu.native.shims import adopt_command_line;"
              r"adopt_command_line(r'C:\Jeux\jeu.exe');"
              "k = ctypes.windll.kernel32;"
              "k.GetCommandLineW.restype = ctypes.c_wchar_p; k.GetCommandLineA.restype = ctypes.c_char_p;"
              "print(k.GetCommandLineW()); print(k.GetCommandLineA().decode())")
    out = subprocess.run([sys.executable, "-c", script, "--argument-long-de-python" * 4],
                         capture_output=True, text=True, check=True).stdout.splitlines()
    assert out == [r'"C:\Jeux\jeu.exe"'] * 2


def test_game_runs_from_its_own_folder(tmp_path, monkeypatch, runtime_dir):
    from emu import native

    monkeypatch.chdir(os.path.dirname(__file__))
    folder = tmp_path / "jeu"
    folder.mkdir()
    # native.run ne libère pas l'image (le jeu termine le processus) : base dédiée.
    exe = build_pe(folder / "answer.exe", lambda iat: Asm().mov_eax(5).ret().build(), dll=False,
                   base=0x1_5000_0000)
    assert native.run(exe, log=lambda *_: None) & 0xFFFF_FFFF == 5
    assert os.path.samefile(os.getcwd(), folder)


def test_console_xmem_import_is_served_by_cpp_runtime(tmp_path, runtime_dir):
    """Le jeu importe xmem.dll!XMemAlloc : la DLL C++ d'AnyXboxX répond."""
    def code(iat):
        return (Asm().frame().mov_ecx(64).zero_rdx()
                .call_iat(iat[("xmem.dll", "XMemAlloc")]).unframe().ret().build())

    exe = build_pe(tmp_path / "mem.exe", code, imports=[("xmem.dll", "XMemAlloc")], dll=False)
    pointer = run(exe)[0]
    assert pointer and pointer % 64 == 0


def test_dll_loaded_by_windows_also_gets_cpp_runtime(tmp_path, runtime_dir):
    """Une DLL du jeu chargée par Windows trouve xmem.dll déjà préchargé par AnyXboxX."""
    build_pe(tmp_path / "helper.dll",
             lambda iat: (Asm().frame().mov_ecx(64).zero_rdx()
                          .call_iat(iat[("xmem.dll", "XMemAlloc")]).unframe().ret()
                          .at(DLL_MAIN).mov_eax(1).ret().build()),
             imports=[("xmem.dll", "XMemAlloc")], exports=[("Alloc", 0)], entry=DLL_MAIN)
    exe = build_pe(tmp_path / "game.exe",
                   lambda iat: Asm().frame().call_iat(iat[("helper.dll", "Alloc")]).unframe().ret().build(),
                   imports=[("helper.dll", "Alloc")], dll=False)
    pointer = run(exe)[0]
    assert pointer and pointer % 64 == 0


def test_untranslated_console_imports_are_listed_together(tmp_path, runtime_dir):
    from emu.native import NativeImage

    imports = [("xmem.dll", "XMemPasEncore"), ("d3d12_xs.dll", "D3D12XboxCreateDevice")]
    exe = build_pe(tmp_path / "todo.exe", lambda iat: Asm().mov_eax(1).ret().build(),
                   imports=imports, dll=False)
    with pytest.raises(ValueError) as error:
        NativeImage(exe)
    assert "xmem.dll!XMemPasEncore" in str(error.value)
    assert "d3d12_xs.dll!D3D12XboxCreateDevice" in str(error.value)
    # L'échec a libéré l'image : un autre programme reprend la même base.
    assert run(build_pe(tmp_path / "next.exe", lambda iat: Asm().mov_eax(4).ret().build(), dll=False))[0] == 4


def test_runtime_is_shared_and_found_by_absolute_path(runtime_dir, monkeypatch):
    from emu.native import runtime

    assert runtime.shared() is runtime.shared()
    monkeypatch.setenv("ANYXBOXX_RUNTIME", "dossier/relatif")
    assert runtime.runtime_dir().is_absolute()


def _iat_slot(module_path, handle, dll, function):
    """Adresse mémoire de l'entrée d'IAT `dll!function` d'un module chargé à `handle`."""
    import pefile

    with pefile.PE(str(module_path)) as pe:
        for entry in pe.DIRECTORY_ENTRY_IMPORT:
            if entry.dll.decode().lower() == dll.lower():
                for item in entry.imports:
                    if item.name and item.name.decode() == function:
                        return handle + item.address - pe.OPTIONAL_HEADER.ImageBase
    raise AssertionError(f"{dll}!{function} absent")


def test_game_registry_reads_go_to_the_simulated_console(tmp_path, runtime_dir):
    from emu.native import NativeImage, runtime

    exe = build_pe(tmp_path / "reg.exe", lambda iat: Asm().mov_eax(1).ret().build(),
                   imports=[("ADVAPI32.dll", "RegQueryValueExW")], dll=False)
    image = NativeImage(exe)
    try:
        assert image.bound["ADVAPI32.dll!RegQueryValueExW"] == runtime.shared().export("AnyXboxX_RegQueryValueExW")
    finally:
        image.close()


def test_registry_imports_of_game_dlls_are_patched_too(tmp_path, runtime_dir):
    """Une DLL du jeu chargée par Windows (ex. SDL3.dll) doit aussi voir une Xbox Series X."""
    import ctypes

    from emu.native import NativeImage, runtime

    helper = build_pe(tmp_path / "regdll.dll", lambda iat: Asm().mov_eax(1).ret().at(DLL_MAIN).mov_eax(1).ret().build(),
                      imports=[("api-ms-win-core-registry-l1-1-0.dll", "RegOpenKeyExW")],
                      exports=[("Nothing", 0)], entry=DLL_MAIN)
    exe = build_pe(tmp_path / "usesdll.exe", lambda iat: Asm().mov_eax(1).ret().build(),
                   imports=[("regdll.dll", "Nothing")], dll=False)
    image = NativeImage(exe)
    try:
        kernel32 = ctypes.WinDLL("kernel32")
        kernel32.GetModuleHandleW.restype = ctypes.c_void_p
        handle = kernel32.GetModuleHandleW(str(helper))
        slot = _iat_slot(helper, handle, "api-ms-win-core-registry-l1-1-0.dll", "RegOpenKeyExW")
        assert ctypes.c_uint64.from_address(slot).value == runtime.shared().export("AnyXboxX_RegOpenKeyExW")
    finally:
        image.close()
