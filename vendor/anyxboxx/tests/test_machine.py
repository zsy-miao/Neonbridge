import struct

import pytest
from unicorn import UC_PROT_ALL
from unicorn.x86_const import UC_X86_REG_RCX, UC_X86_REG_RDX, UC_X86_REG_R8

from emu import hle
from emu.hle.guest import INITTERM, INITTERM_E
from emu.machine import TEB, Machine
from emu.report import analyse
from scarlett import DEMO, WIDTH, HEIGHT

CODE = 0x5000_0000


@pytest.fixture
def machine():
    return Machine(DEMO, title_id=0x1234)


def test_demo_detects_series_x(machine):
    assert machine.start() == 7


def test_demo_draws_a_frame(machine):
    machine.start()
    pixels = machine.heap.alloc(WIDTH * HEIGHT * 3)
    machine.call(machine.image.exports["DrawFrame"], pixels, 0, 0)
    assert bytes(machine.cpu.mem_read(pixels, 3)) == bytes.fromhex("302653")


def test_teb_is_reachable_through_gs(machine):
    assert struct.unpack("<Q", machine.cpu.mem_read(TEB + 0x30, 8))[0] == TEB
    assert struct.unpack("<Q", machine.cpu.mem_read(TEB + 0x58, 8))[0] == machine.tls_array


def counter_code(machine, address, value):
    """Fonction invitée : *address += 1 ; renvoie value."""
    return (b"\x48\xb8" + struct.pack("<Q", address) + b"\xff\x00"
            + b"\xb8" + struct.pack("<I", value) + b"\xc3")


def setup_initterm(machine, routine, results):
    machine.cpu.mem_map(CODE, 0x1000, UC_PROT_ALL)
    counter = machine.heap.alloc(8)
    machine.cpu.mem_write(CODE, routine)
    pointers = []
    for i, value in enumerate(results):
        fn = CODE + 0x100 + 0x40 * i
        machine.cpu.mem_write(fn, counter_code(machine, counter, value))
        pointers.append(fn)
    table = machine.heap.alloc(8 * (len(pointers) + 1))
    machine.cpu.mem_write(table, b"".join(struct.pack("<Q", p) for p in pointers[:1])
                          + b"\0" * 8 + b"".join(struct.pack("<Q", p) for p in pointers[1:]))
    return counter, table, table + 8 * (len(pointers) + 1)


def read_u32(machine, address):
    return struct.unpack("<I", machine.cpu.mem_read(address, 4))[0]


def test_initterm_calls_every_constructor(machine):
    counter, begin, end = setup_initterm(machine, INITTERM, [5, 6, 7])
    assert machine.call(CODE, begin, end) == 0
    assert read_u32(machine, counter) == 3  # le pointeur nul est ignoré


def test_initterm_e_stops_on_error(machine):
    counter, begin, end = setup_initterm(machine, INITTERM_E, [0, 9, 0])
    assert machine.call(CODE, begin, end) == 9
    assert read_u32(machine, counter) == 2


def invoke(machine, dll, name, *args):
    for register, value in zip((UC_X86_REG_RCX, UC_X86_REG_RDX, UC_X86_REG_R8), args, strict=False):
        machine.cpu.reg_write(register, value)
    kind, handler = hle.resolve(dll, name)
    assert kind == "python"
    return handler(hle.Call(machine))


def test_console_id_follows_gdk_buffer_contract(machine):
    buffer, used = machine.heap.alloc(64), machine.heap.alloc(8)
    assert invoke(machine, "xgameruntime.dll", "XSystemGetConsoleId", 64, buffer, used) == 0
    size = struct.unpack("<Q", machine.cpu.mem_read(used, 8))[0]
    assert size == 39
    assert bytes(machine.cpu.mem_read(buffer, size)) == b"00000000.00000000.00000000.00000000.00\0"
    result = invoke(machine, "xgameruntime.dll", "XSystemGetConsoleId", 4, buffer, used)
    assert result == hle.core.E_INVALIDARG


def test_title_id_comes_from_manifest(machine):
    out = machine.heap.alloc(4)
    invoke(machine, "xgameruntime.dll", "XGameGetXboxTitleId", out)
    assert read_u32(machine, out) == 0x1234


def test_tls_slots_roundtrip(machine):
    index = invoke(machine, "kernel32.dll", "TlsAlloc")
    assert invoke(machine, "api-ms-win-core-processthreads-l1-1-0.dll", "TlsSetValue", index, 42) == 1
    assert invoke(machine, "kernelx.dll", "TlsGetValue", index) == 42


def test_heap_realloc_keeps_content(machine):
    first = invoke(machine, "ucrtbase.dll", "malloc", 4)
    machine.cpu.mem_write(first, b"Xbox")
    second = invoke(machine, "ucrtbase.dll", "realloc", first, 64)
    assert bytes(machine.cpu.mem_read(second, 4)) == b"Xbox"


def test_unknown_dll_has_no_handler():
    assert hle.resolve("d3d12_xs.dll", "D3D12CreateDevice") == ("absent", None)


def test_report_on_demo():
    report = analyse(DEMO)
    assert report.total == 1 and report.coverage == 100.0
    assert "XSystemGetDeviceType" in report.emulated[0]
