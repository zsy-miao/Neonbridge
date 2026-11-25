"""Chemin de démarrage GDK d'un titre (docs/research/first-boot-1021.md, section 3, étapes 6 à 9).

Le programme de test refait ce que fait le chargeur de xgameruntime.lib lié dans le titre :
GetModuleFileNameW(NULL), nom de fichier remplacé par « xgameruntime.dll », chargement par
chemin complet (mode local, 0xA00) puis par nom (0x800), six GetProcAddress, appel des
initialiseurs avec les bits d'options 2, 4 et 8. Le titre est dans un autre dossier que le
runtime : AnyXboxX doit quand même lui servir son propre xgameruntime.dll.
"""
import os
import subprocess
import sys

import pytest

from pe_builder import Asm, build_pe

pytestmark = pytest.mark.skipif(sys.platform != "win32", reason="exécution native sous Windows")

GDK_NAMES = ("QueryApiImpl", "XErrorReport", "UninitializeApiImpl",
             "InitializeApiImpl", "InitializeApiImplEx", "InitializeApiImplEx2")
INITIALIZERS = GDK_NAMES[3:]
# Étape 8 : deux constantes 64 bits puis un mot d'options auquel le titre ajoute 2, 4 et 8.
# L'ordre des arguments (rcx, rdx, r8) est supposé.
CONSTANTS = (0x633600002712, 0x65F400001EDD)
OPTIONS = 2 | 4 | 8
LOAD_SYSTEM32, LOAD_LOCAL = 0x800, 0xA00
TITLE = "title_w9_gdk.exe"          # 16 caractères, comme « xgameruntime.dll »
RUNTIME_NAME = "xgameruntime.dll"
PATH_CHARS = 0x100                  # tampon de chemin sur la pile, à [rsp+0x20]
WIDE_RUNTIME, NAMES, SLOTS = 0x200, 0x230, 0x2C0
SAME_MODULE = 1 << 6
INITIALIZED = 0x7 << 8
EXPECTED = (1 << len(GDK_NAMES)) - 1 | SAME_MODULE | INITIALIZED
CALL_BLOCK = 41                     # octets d'un appel d'initialiseur, sautés si GetProcAddress a échoué


def _accumulate(asm, condition, bit):
    """ebx |= (condition sur le dernier test) << bit, sans saut."""
    return (asm.raw(condition + b"\x0f\xb6\xc0")            # setcc al ; movzx eax, al
            .raw(b"\xc1\xe0" + bytes([bit])).raw(b"\x09\xc3"))   # shl eax, bit ; or ebx, eax


def _name_offsets():
    offsets, cursor = {}, NAMES
    for name in GDK_NAMES:
        offsets[name] = cursor
        cursor += len(name) + 1
    return offsets


def gdk_loader(iat):
    names = _name_offsets()
    asm = Asm().raw(b"\x53\x56\x57").raw(b"\x48\x81\xec\x30\x02\x00\x00").raw(b"\x31\xdb")  # push ; sub rsp
    # GetModuleFileNameW(NULL, buffer, 0x100), puis les 16 derniers caractères remplacés.
    asm.zero_rcx().raw(b"\x48\x8d\x54\x24\x20").mov_r8d(PATH_CHARS)
    asm.call_iat(iat[("kernel32.dll", "GetModuleFileNameW")])
    asm.raw(b"\x89\xc0").raw(b"\x48\x8d\x3c\x44")            # mov eax, eax ; lea rdi, [rsp+rax*2]
    asm.rip(b"\x48\x8d\x35", WIDE_RUNTIME).mov_ecx(2 * len(RUNTIME_NAME)).raw(b"\xf3\xa4")  # rep movsb
    # Chargement par chemin complet (dossier du titre), puis par nom.
    asm.raw(b"\x48\x8d\x4c\x24\x20").zero_rdx().mov_r8d(LOAD_LOCAL)
    asm.call_iat(iat[("kernel32.dll", "LoadLibraryExW")]).raw(b"\x48\x89\xc6")       # mov rsi, rax
    asm.lea_rcx(WIDE_RUNTIME).zero_rdx().mov_r8d(LOAD_SYSTEM32)
    asm.call_iat(iat[("kernel32.dll", "LoadLibraryExW")]).raw(b"\x48\x89\xc7")       # mov rdi, rax
    _accumulate(asm.raw(b"\x48\x39\xfe"), b"\x0f\x94\xc0", 6)                        # cmp rsi, rdi ; sete
    for bit, name in enumerate(GDK_NAMES):
        asm.raw(b"\x48\x89\xf1").lea_rdx(names[name]).call_iat(iat[("kernel32.dll", "GetProcAddress")])
        if name in INITIALIZERS:
            asm.rip(b"\x48\x89\x05", SLOTS + 8 * INITIALIZERS.index(name))           # mov [slot], rax
        _accumulate(asm.raw(b"\x48\x85\xc0"), b"\x0f\x95\xc0", bit)                  # test rax ; setnz
    for index in range(len(INITIALIZERS)):
        asm.rip(b"\x48\x8b\x05", SLOTS + 8 * index)                                  # mov rax, [slot]
        asm.raw(b"\x48\x85\xc0\x74" + bytes([CALL_BLOCK]))                           # absent : pas d'appel
        start = asm.here
        asm.raw(b"\x48\xb9" + CONSTANTS[0].to_bytes(8, "little"))                    # mov rcx, imm64
        asm.raw(b"\x48\xba" + CONSTANTS[1].to_bytes(8, "little")).mov_r8d(OPTIONS)
        asm.call_rax()
        _accumulate(asm.raw(b"\x85\xc0"), b"\x0f\x94\xc0", 8 + index)                # S_OK ?
        assert asm.here - start == CALL_BLOCK
    asm.raw(b"\x89\xd8").raw(b"\x48\x81\xc4\x30\x02\x00\x00").raw(b"\x5f\x5e\x5b").ret()
    asm.at(WIDE_RUNTIME).raw(RUNTIME_NAME.encode("utf-16-le") + b"\0\0")
    for name in GDK_NAMES:
        asm.at(names[name]).raw(name.encode() + b"\0")
    return asm.at(SLOTS).raw(bytes(8 * len(INITIALIZERS))).build()


GDK_IMPORTS = [("kernel32.dll", "GetModuleFileNameW"), ("kernel32.dll", "LoadLibraryExW"),
               ("kernel32.dll", "GetProcAddress")]


@pytest.fixture
def title_elsewhere(tmp_path, runtime_dir):
    """Le titre dans son propre dossier, sans xgameruntime.dll à côté, loin du runtime."""
    folder = tmp_path / "titre"
    folder.mkdir()
    assert not (folder / RUNTIME_NAME).exists() and folder.resolve() != runtime_dir.resolve()
    return build_pe(folder / TITLE, gdk_loader, imports=GDK_IMPORTS, dll=False, base=0x1_5800_0000)


def test_launcher_serves_the_gdk_loader_of_a_title_in_another_folder(title_elsewhere, runtime_dir):
    environment = {**os.environ, "ANYXBOXX_SYSTEM_RUNTIME": "0"}   # jamais le runtime de Windows ici
    result = subprocess.run([str(runtime_dir / "anyxboxx.exe"), str(title_elsewhere)], capture_output=True,
                            text=True, timeout=30, env=environment)
    assert result.returncode == EXPECTED, (hex(result.returncode), result.stderr)


@pytest.mark.skipif(os.environ.get("ANYXBOXX_SYSTEM_RUNTIME") != "0",
                    reason="le runtime GDK de Windows recevrait les constantes du test")
def test_native_mode_serves_the_same_gdk_loader(title_elsewhere):
    from emu.native import NativeImage

    image = NativeImage(title_elsewhere)
    try:
        assert image.call_entry() & 0xFFFF_FFFF == EXPECTED
    finally:
        image.close()


# Étape 9 : le titre lit HKLM\...\CurrentVersion\OEM, DWORD ConsoleMode, par RegOpenKeyW (pas Ex).
# 0x64 se convertit en 7 (XboxScarlettAnaconda) dans xgameruntime.lib : seules 1, 6, 7 et 8 passent.
SCARLETT_CONSOLE_MODE = 0x64
OEM_KEY = r"Software\Microsoft\Windows NT\CurrentVersion\OEM"
WIDE_KEY, WIDE_VALUE = 0x100, 0x200
REGISTRY_IMPORTS = [("ADVAPI32.dll", "RegOpenKeyW"), ("ADVAPI32.dll", "RegQueryValueExW"),
                    ("ADVAPI32.dll", "RegCloseKey")]


def console_mode_reader(iat):
    """RegOpenKeyW(HKLM, OEM, &key) ; RegQueryValueExW(key, "ConsoleMode", …) ; rend la valeur lue (0 sinon)."""
    asm = Asm().raw(b"\x48\x83\xec\x48")                                     # sub rsp, 0x48
    asm.raw(b"\x48\xc7\x44\x24\x30" + bytes(4)).raw(b"\xc7\x44\x24\x38" + bytes(4))   # key = 0, valeur = 0
    asm.raw(b"\x48\xc7\xc1\x02\x00\x00\x80").lea_rdx(WIDE_KEY)               # rcx = HKEY_LOCAL_MACHINE
    asm.raw(b"\x4c\x8d\x44\x24\x30").call_iat(iat[REGISTRY_IMPORTS[0]])      # r8 = &key
    asm.raw(b"\x48\x8b\x4c\x24\x30").lea_rdx(WIDE_VALUE).raw(b"\x45\x31\xc0\x45\x31\xc9")
    asm.raw(b"\x48\x8d\x44\x24\x38\x48\x89\x44\x24\x20")                    # 5e argument : &valeur
    asm.raw(b"\x48\x8d\x44\x24\x3c\x48\x89\x44\x24\x28")                    # 6e argument : &taille
    asm.raw(b"\xc7\x44\x24\x3c\x04\x00\x00\x00").call_iat(iat[REGISTRY_IMPORTS[1]])
    asm.raw(b"\x48\x8b\x4c\x24\x30").call_iat(iat[REGISTRY_IMPORTS[2]])
    asm.raw(b"\x8b\x44\x24\x38\x48\x83\xc4\x48").ret()                      # mov eax, valeur
    asm.at(WIDE_KEY).raw(OEM_KEY.encode("utf-16-le") + b"\0\0")
    return asm.at(WIDE_VALUE).raw("ConsoleMode".encode("utf-16-le") + b"\0\0").build()


@pytest.fixture
def console_mode_title(tmp_path, runtime_dir):
    return build_pe(tmp_path / "consolemode.exe", console_mode_reader, imports=REGISTRY_IMPORTS, dll=False,
                    base=0x1_5900_0000)


def test_launcher_answers_console_mode_through_reg_open_key(console_mode_title, runtime_dir):
    result = subprocess.run([str(runtime_dir / "anyxboxx.exe"), str(console_mode_title)], capture_output=True,
                            text=True, timeout=30)
    assert result.returncode == SCARLETT_CONSOLE_MODE, (hex(result.returncode), result.stderr)


def test_native_mode_answers_console_mode_through_reg_open_key(console_mode_title):
    from emu.native import NativeImage

    image = NativeImage(console_mode_title)
    try:
        assert image.call_entry() & 0xFFFF_FFFF == SCARLETT_CONSOLE_MODE
    finally:
        image.close()
