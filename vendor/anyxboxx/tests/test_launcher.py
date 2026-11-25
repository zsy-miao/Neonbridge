"""anyxboxx.exe : le lanceur natif en C++, sans Python dans le processus du jeu."""
import ctypes
import os
import subprocess
import sys

import pytest

from emu import tombox_paths
from pe_builder import Asm, build_pe
from test_native_tls import CALLBACKS, DIRECTORY, INDEX, TEMPLATE, VALUE, tls_reader

pytestmark = pytest.mark.skipif(sys.platform != "win32", reason="exécution native sous Windows")


def launch(runtime_dir, target):
    return subprocess.run([str(runtime_dir / "anyxboxx.exe"), str(target)], capture_output=True, text=True,
                          timeout=30)


def test_entry_point_result_is_the_exit_code(tmp_path, runtime_dir):
    exe = build_pe(tmp_path / "answer.exe", lambda iat: Asm().mov_eax(42).ret().build(), dll=False)
    assert launch(runtime_dir, exe).returncode == 42


def test_console_memory_import_is_served_by_the_runtime(tmp_path, runtime_dir):
    def code(iat):
        return Asm().frame().mov_ecx(64).zero_rdx().call_iat(iat[("xmem.dll", "XMemAlloc")]).unframe().ret().build()

    exe = build_pe(tmp_path / "mem.exe", code, imports=[("xmem.dll", "XMemAlloc")], dll=False)
    pointer = launch(runtime_dir, exe).returncode & 0xFFFF_FFFF
    assert pointer != 0 and pointer % 64 == 0


def test_thread_local_works_without_python(tmp_path, runtime_dir):
    pointers = {DIRECTORY: TEMPLATE, DIRECTORY + 8: TEMPLATE + 4, DIRECTORY + 16: INDEX, DIRECTORY + 24: CALLBACKS}
    exe = build_pe(tmp_path / "tls.exe", lambda iat: tls_reader(), dll=False, pointers=pointers, tls=DIRECTORY)
    assert launch(runtime_dir, exe).returncode & 0xFFFF_FFFF == VALUE


def test_package_folder_launches_its_declared_executable(tmp_path, runtime_dir):
    build_pe(tmp_path / "game.exe", lambda iat: Asm().mov_eax(7).ret().build(), dll=False)
    (tmp_path / "MicrosoftGame.config").write_text(
        '<Game><ExecutableList><Executable Name="game.exe" TargetDeviceFamily="Scarlett" /></ExecutableList></Game>',
        encoding="utf-8")
    assert launch(runtime_dir, tmp_path).returncode == 7


def test_missing_imports_are_named(tmp_path, runtime_dir):
    exe = build_pe(tmp_path / "todo.exe", lambda iat: Asm().mov_eax(1).ret().build(),
                   imports=[("d3d12_xs.dll", "D3D12XboxCreateDevice")], dll=False)
    result = launch(runtime_dir, exe)
    assert result.returncode == 1 and "d3d12_xs.dll!D3D12XboxCreateDevice" in result.stderr


def test_package_without_its_executable_says_so(tmp_path, runtime_dir):
    (tmp_path / "MicrosoftGame.config").write_text(
        '<Game><ExecutableList><Executable Name="game_scarlett_master.exe" /></ExecutableList></Game>',
        encoding="utf-8")
    result = subprocess.run([str(runtime_dir / "anyxboxx.exe"), str(tmp_path)], capture_output=True, timeout=30)
    stderr = result.stderr.decode("utf-8", errors="replace") + result.stderr.decode("mbcs", errors="replace")
    assert result.returncode == 1 and "game_scarlett_master.exe" in stderr


SEM_NOGPFAULTERRORBOX = 0x0002


def launch_without_crash_window(runtime_dir, target):
    """Comme launch, sans la fenêtre de rapport d'erreurs de Windows : l'enfant hérite du mode d'erreur."""
    kernel32 = ctypes.windll.kernel32
    previous = kernel32.GetErrorMode()
    kernel32.SetErrorMode(previous | SEM_NOGPFAULTERRORBOX)
    try:
        return launch(runtime_dir, target)
    finally:
        kernel32.SetErrorMode(previous)


def test_first_exception_names_the_module_and_rva(tmp_path, runtime_dir):
    exe = build_pe(tmp_path / "trap.exe", lambda iat: Asm().mov_eax(1).raw(b"\xcc").ret().build(), dll=False)
    result = launch_without_crash_window(runtime_dir, exe)          # int3 à la RVA 0x1005, après mov eax, 1
    assert result.returncode & 0xFFFF_FFFF == 0x8000_0003
    assert "0x80000003" in result.stderr and "trap.exe+0x1005" in result.stderr


# W13 (first-boot-1021.md) : le titre importe GameInput.dll!#1, le GameInputCreate v0 du GameInput.lib
# du GDK. AnyXboxX ne le sert pas : la DLL de l'hôte le fournit, sinon le lanceur nomme l'import manquant.
GAME_INPUT_OUT = 0x300


def _host_game_input_create():
    from test_native_services import windows_export

    return windows_export("GameInput.dll", "GameInputCreate")


@pytest.mark.skipif(sys.platform != "win32" or not _host_game_input_create(), reason="GameInput absent de cet hôte")
def test_game_input_ordinal_1_is_the_host_game_input_create(tmp_path, runtime_dir):
    def code(iat):
        return (Asm().frame().lea_rcx(GAME_INPUT_OUT).call_iat(iat[("GameInput.dll", 1)])
                .raw(b"\x89\xc2").mov_rax_from(GAME_INPUT_OUT).nonzero_rax_to_eax()     # edx = HRESULT
                .raw(b"\x09\xd0").unframe().ret()                                       # 1 : S_OK et objet
                .at(GAME_INPUT_OUT).raw(bytes(8)).build())

    exe = build_pe(tmp_path / "input.exe", code, imports=[("GameInput.dll", 1)], dll=False)
    result = launch(runtime_dir, exe)
    assert result.returncode == 1, (hex(result.returncode & 0xFFFF_FFFF), result.stderr)


def test_host_library_missing_an_ordinal_import_is_named(tmp_path, runtime_dir):
    exe = build_pe(tmp_path / "noinput.exe", lambda iat: Asm().mov_eax(7).ret().build(),
                   imports=[("AnyXboxXHostAbsent.dll", 1)], dll=False)
    result = launch(runtime_dir, exe)
    assert result.returncode == 1 and "AnyXboxXHostAbsent.dll!#1" in result.stderr


TOMBOX_D3D12 = tombox_paths.TOMBOX_INSTALL / "d3d12_xs.dll"


@pytest.mark.skipif(not TOMBOX_D3D12.is_file(), reason="d3d12_xs de TomBox non compilée (python tools/build_tombox.py)")
def test_tombox_d3d12_serves_console_device_creation(tmp_path, runtime_dir):
    exe = build_pe(tmp_path / "d3d.exe", lambda iat: Asm().mov_eax(7).ret().build(),
                   imports=[("d3d12_xs.dll", "D3D12XboxCreateDevice"), ("d3d12_xs.dll", "DXGIXPresentArray")], dll=False)
    assert launch(runtime_dir, exe).returncode == 1                  # absent de notre d3d12_xs
    environment = {**os.environ, "ANYXBOXX_D3D12_XS": str(TOMBOX_D3D12)}
    result = subprocess.run([str(runtime_dir / "anyxboxx.exe"), str(exe)], capture_output=True, text=True,
                            timeout=60, env=environment)
    assert result.returncode == 7, result.stderr


TOMBOX_DIR = TOMBOX_D3D12.parent


@pytest.mark.skipif(not (TOMBOX_DIR / "xg_xs.dll").is_file() or not (TOMBOX_DIR / "xmem.dll").is_file(),
                    reason="DLL de TomBox non compilées (tools/build_tombox.py xg_xs xmem)")
def test_tombox_dir_serves_missing_console_libraries(tmp_path, runtime_dir):
    def code(iat):
        return Asm().frame().mov_ecx(64).zero_rdx().call_iat(iat[("xmem.dll", "XMemAlloc")]).unframe().ret().build()

    imports = [("xmem.dll", "XMemAlloc"), ("xmem.dll", "XMemGetAllocationStatistics"),
               ("xg_xs.dll", "XGComputeTexture2DLayout")]
    exe = build_pe(tmp_path / "xg.exe", code, imports=imports, dll=False)
    assert launch(runtime_dir, exe).returncode == 1                  # xg_xs absent d'AnyXboxX
    environment = {**os.environ, "ANYXBOXX_TOMBOX_DIR": str(TOMBOX_DIR)}
    result = subprocess.run([str(runtime_dir / "anyxboxx.exe"), str(exe)], capture_output=True, text=True,
                            timeout=60, env=environment)
    pointer = result.returncode & 0xFFFF_FFFF                        # XMemAlloc de TomBox : alignement par défaut 16
    assert pointer not in (0, 1) and pointer % 16 == 0, result.stderr


NEW_TOMBOX_LIBRARIES = {"d3d12_x.dll": "D3D12XboxCreateDevice", "xapu.dll": "XApuGetUtilization",
                        "xgs12.dll": "OpenAdapter12"}


@pytest.mark.skipif(not all((TOMBOX_DIR / name).is_file() for name in NEW_TOMBOX_LIBRARIES),
                    reason="DLL de TomBox non compilées (tools/build_tombox.py d3d12_x xapu xgs12)")
def test_tombox_dir_serves_esram_apu_and_umd_libraries(tmp_path, runtime_dir):
    imports = list(NEW_TOMBOX_LIBRARIES.items())
    exe = build_pe(tmp_path / "libs.exe", lambda iat: Asm().mov_eax(42).ret().build(), imports=imports, dll=False)
    assert launch(runtime_dir, exe).returncode != 42                 # bibliothèques absentes d'AnyXboxX
    environment = {**os.environ, "ANYXBOXX_TOMBOX_DIR": str(TOMBOX_DIR)}
    result = subprocess.run([str(runtime_dir / "anyxboxx.exe"), str(exe)], capture_output=True, text=True,
                            timeout=60, env=environment)
    assert result.returncode == 42, result.stderr


AUDIO_TOMBOX_LIBRARIES = {"AcpHal.dll": "AcpHalCreate", "chad.dll": "XDspConnectWithMaximumStreamLimit"}


@pytest.mark.skipif(not all((TOMBOX_DIR / name.lower()).is_file() for name in AUDIO_TOMBOX_LIBRARIES),
                    reason="DLL audio de TomBox non compilées (tools/build_tombox.py acphal chad)")
def test_tombox_dir_serves_acphal_and_chad_audio_libraries(tmp_path, runtime_dir):
    imports = list(AUDIO_TOMBOX_LIBRARIES.items())
    exe = build_pe(tmp_path / "audio_libs.exe", lambda iat: Asm().mov_eax(42).ret().build(), imports=imports, dll=False)
    assert launch(runtime_dir, exe).returncode != 42
    environment = {**os.environ, "ANYXBOXX_TOMBOX_DIR": str(TOMBOX_DIR)}
    result = subprocess.run([str(runtime_dir / "anyxboxx.exe"), str(exe)], capture_output=True, text=True,
                            timeout=60, env=environment)
    assert result.returncode == 42, result.stderr
