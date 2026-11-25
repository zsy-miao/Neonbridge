"""Démarrage d'un .exe : CRT, texte, kernel32 courant et interface interne du GDK PC."""
import struct

import pytest

from emu import bench, hle
from emu.machine import Machine
from emu.report import analyse
from pe_builder import Asm, build_pe
from scarlett import DEMO


@pytest.fixture
def m():
    return Machine(DEMO)


def call(machine, dll, name, *args):
    """Convention Windows x64 : 4 registres, puis la pile après 32 octets réservés."""
    from unicorn.x86_const import UC_X86_REG_RSP
    for register, value in zip(hle.core.ARG_REGISTERS, args, strict=False):
        machine.cpu.reg_write(register, value)
    frame = machine.heap.alloc(0x100)
    machine.cpu.reg_write(UC_X86_REG_RSP, frame)
    for index, value in enumerate(args[4:]):
        machine.cpu.mem_write(frame + 8 + 32 + 8 * index, struct.pack("<Q", value))
    return hle.resolve(dll, name)[1](hle.Call(machine))


def text(machine, value, wide=False):
    data = value.encode("utf-16-le") + b"\0\0" if wide else value.encode() + b"\0"
    address = machine.heap.alloc(len(data) + 16)
    machine.cpu.mem_write(address, data)
    return address


def u64(machine, address):
    return struct.unpack("<Q", machine.cpu.mem_read(address, 8))[0]


CRT = "api-ms-win-crt-runtime-l1-1-0.dll"


def test_crt_globals_are_stable_pointers(m):
    first = call(m, CRT, "_errno")
    assert first and call(m, CRT, "_errno") == first
    assert call(m, "ucrtbase.dll", "__p__commode") != first


def test_argv_points_to_program_name(m):
    argv = u64(m, call(m, CRT, "__p___argv"))
    name = u64(m, argv)
    assert bytes(m.cpu.mem_read(name, 8)) == b"game.exe" and u64(m, argv + 8) == 0


def test_localeconv_decimal_point(m):
    lconv = call(m, "ucrtbase.dll", "localeconv")
    assert bytes(m.cpu.mem_read(u64(m, lconv), 2)) == b".\0"


def test_strtoll_and_end_pointer(m):
    source, end = text(m, "  -42xyz"), m.heap.alloc(8)
    assert call(m, "ucrtbase.dll", "strtoll", source, end, 10) == (-42) & 0xFFFF_FFFF_FFFF_FFFF
    assert u64(m, end) == source + 5


def test_strtoull_hexadecimal(m):
    assert call(m, "ucrtbase.dll", "strtoull", text(m, "0x1F"), 0, 0) == 31


def test_strtod_writes_xmm0(m):
    from unicorn.x86_const import UC_X86_REG_XMM0
    call(m, "ucrtbase.dll", "strtod", text(m, "2.5e1"), 0)
    raw = m.cpu.reg_read(UC_X86_REG_XMM0) & 0xFFFF_FFFF_FFFF_FFFF
    assert struct.unpack("<d", struct.pack("<Q", raw))[0] == 25.0


def test_wcscat_s(m):
    dest = text(m, "Xbox", wide=True)
    assert call(m, "ucrtbase.dll", "wcscat_s", dest, 16, text(m, " X", wide=True)) == 0
    assert bytes(m.cpu.mem_read(dest, 14)).decode("utf-16-le") == "Xbox X\0"
    assert call(m, "ucrtbase.dll", "wcscat_s", dest, 4, text(m, "long", wide=True)) == 34


def test_wide_char_to_multibyte_utf8(m):
    source, out = text(m, "é", wide=True), m.heap.alloc(8)
    count = call(m, "kernel32.dll", "WideCharToMultiByte", 65001, 0, source, 0xFFFF_FFFF, out, 8)
    assert count == 3 and bytes(m.cpu.mem_read(out, 3)) == "é\0".encode()


def test_events_and_waits(m):
    event = call(m, "kernel32.dll", "CreateEventExW", 0, 0, 0, 0)
    assert m.handles[event] == "event"
    assert call(m, "kernel32.dll", "WaitForSingleObjectEx", event, 1000, 0) == 0
    assert call(m, "kernel32.dll", "CloseHandle", event) == 1


def test_message_box_goes_to_log(m):
    call(m, "user32.dll", "MessageBoxW", 0, text(m, "Bonjour", True), text(m, "Titre", True), 0)
    assert m.logs[-1] == "[MessageBox] Titre : Bonjour"


def test_gdk_pc_internal_api(m):
    assert call(m, "xgameruntime.dll", "InitializeApiImplEx2", 0, 0) == 0 and m.runtime_ready
    out = m.heap.alloc(8)
    m.cpu.mem_write(out, b"\xff" * 8)
    assert call(m, "xgameruntime.dll", "QueryApiImpl", 0, 0, out) == 0x80004002
    assert u64(m, out) == 0


def test_extra_dll_folder_is_searched(tmp_path):
    lib_dir, game_dir = tmp_path / "redist", tmp_path / "jeu"
    lib_dir.mkdir()
    game_dir.mkdir()
    build_pe(lib_dir / "helper.dll",
             lambda iat: Asm().mov_eax(1).ret().at(0x10).mov_eax(7).ret().build(),
             exports=[("Seven", 0x10)])
    build_pe(game_dir / "game.exe",
             lambda iat: Asm().frame().call_iat(iat[("helper.dll", "Seven")]).unframe().ret().build(),
             imports=[("helper.dll", "Seven")], dll=False)
    assert analyse(game_dir / "game.exe").missing == ("helper.dll!Seven",)
    assert analyse(game_dir / "game.exe", (lib_dir,)).provided == ("helper.dll!Seven",)
    row = bench.run_one(game_dir / "game.exe", (lib_dir,))
    assert row.initialised and "retour 0x7" in row.detail


def test_command_line_names_the_program(m):
    wide = call(m, "KERNEL32.dll", "GetCommandLineW")
    narrow = call(m, "kernelx.dll", "GetCommandLineA")
    assert m.cpu.mem_read(wide, 22).decode("utf-16-le") == '"demo.pe"\0\0'[:11]
    assert bytes(m.cpu.mem_read(narrow, 10)) == b'"demo.pe"\0'
    assert call(m, "KERNEL32.dll", "GetCommandLineW") == wide   # même tampon à chaque appel


def test_command_line_to_argv_splits_quoted_words(m):
    count = m.heap.alloc(8)
    argv = call(m, "SHELL32.dll", "CommandLineToArgvW", text(m, r'"C:\a b\jeu.exe" -x  "deux mots"', True), count)
    assert u64(m, count) & 0xFFFF_FFFF == 3
    words = [hle.Call(m).wstr(u64(m, argv + 8 * i)) for i in range(3)]
    assert words == [r"C:\a b\jeu.exe", "-x", "deux mots"]
    assert u64(m, argv + 24) == 0


def test_environment_block_is_empty_and_freeable(m):
    block = call(m, "KERNEL32.dll", "GetEnvironmentStringsW")
    assert bytes(m.cpu.mem_read(block, 4)) == b"\0\0\0\0"
    assert call(m, "kernelx.dll", "FreeEnvironmentStringsW", block) == 1


def test_xmem_allocates_aligned_blocks_in_isolated_mode(m):
    small = call(m, "xmem.dll", "XMemAlloc", 100, 0)
    large = call(m, "xmem.dll", "XMemAlloc", 8192, 0)
    assert small % 64 == 0 and large % 4096 == 0 and small != large
    call(m, "xmem.dll", "XMemFree", small, 0)
    assert call(m, "xmem.dll", "XMemGetWorkingSetStatistics", 0, m.heap.alloc(64)) == 0


def test_xmem_decodes_era_attributes_in_isolated_mode(m):
    """Mêmes règles que runtime/xmem/era_heap.cpp : alignement demandé, physique refusé."""
    heap_64k = 16 << 24                                   # tas, alignement 2^16
    amd_word = 0xA800_4000                                # type graphique 5, pages de 64 Kio
    assert call(m, "xmem.dll", "XMemAlloc", 257, heap_64k) % 0x10000 == 0
    assert call(m, "xmem.dll", "XMemAlloc", 512, amd_word) % 0x10000 == 0
    assert call(m, "xmem.dll", "XMemAlloc", 64, 7 << 29) == 0 and m.last_error == 50        # physique
    assert call(m, "xmem.dll", "XMemAlloc", 64, 0x05E5_0000 | 1 << 40) == 0 and m.last_error == 87
