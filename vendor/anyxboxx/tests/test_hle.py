import struct

import pytest
from unicorn.x86_const import UC_X86_REG_R8, UC_X86_REG_R9, UC_X86_REG_RCX, UC_X86_REG_RDX

from emu import hle
from emu.hle.core import E_NOTIMPL, family
from emu.machine import Machine
from emu.report import Report
from scarlett import DEMO

REGISTERS = (UC_X86_REG_RCX, UC_X86_REG_RDX, UC_X86_REG_R8, UC_X86_REG_R9)


@pytest.fixture
def m():
    return Machine(DEMO)


def call(machine, dll, name, *args):
    for register, value in zip(REGISTERS, args, strict=False):
        machine.cpu.reg_write(register, value)
    return hle.resolve(dll, name)[1](hle.Call(machine))


def u64(machine, address):
    return struct.unpack("<Q", machine.cpu.mem_read(address, 8))[0]


@pytest.mark.parametrize("dll,expected", [
    ("XGameRuntime.dll", "gdk"), ("kernelx.dll", "kernel"),
    ("api-ms-win-core-synch-l1-2-0.dll", "kernel"), ("vcruntime140_1.dll", "crt"),
    ("api-ms-win-crt-heap-l1-1-0.dll", "crt"), ("d3d12_xs.dll", "d3d12_xs"),
])
def test_dll_families(dll, expected):
    assert family(dll) == expected


def test_memory_functions(m):
    a, b = m.heap.alloc(16), m.heap.alloc(16)
    assert call(m, "ucrtbase.dll", "memset", a, 0x41, 4) == a
    call(m, "ucrtbase.dll", "memcpy", b, a, 4)
    assert bytes(m.cpu.mem_read(b, 4)) == b"AAAA"
    assert call(m, "ucrtbase.dll", "memcmp", a, b, 4) == 0
    assert call(m, "ucrtbase.dll", "strlen", a) == 4
    m.cpu.mem_write(b, "Xb".encode("utf-16-le") + b"\0\0")
    assert call(m, "ucrtbase.dll", "wcslen", b) == 2


def test_huge_copy_is_refused(m):
    with pytest.raises(ValueError, match="refusée"):
        call(m, "ucrtbase.dll", "memset", m.heap.alloc(8), 0, 1 << 40)


def test_clock_functions(m):
    out = m.heap.alloc(8)
    assert call(m, "kernel32.dll", "QueryPerformanceFrequency", out) == 1
    assert u64(m, out) == 10_000_000
    call(m, "kernel32.dll", "GetSystemTimeAsFileTime", out)
    assert u64(m, out) > 116_444_736_000_000_000


def test_last_error_and_module_handle(m):
    call(m, "kernel32.dll", "SetLastError", 5)
    assert call(m, "kernel32.dll", "GetLastError") == 5
    assert call(m, "kernel32.dll", "GetModuleHandleW", 0) == m.image_base


def test_exit_process_stops_machine(m):
    call(m, "kernel32.dll", "ExitProcess", 3)
    assert m.exit_code == 3


def test_task_queue_handles(m):
    out = m.heap.alloc(8)
    assert call(m, "xgameruntime.dll", "XTaskQueueCreate", 0, 0, out) == 0
    handle = u64(m, out)
    assert m.handles[handle] == "XTaskQueue"
    call(m, "xgameruntime.dll", "XTaskQueueCloseHandle", handle)
    assert handle not in m.handles


def test_analytics_info_reports_series_x(m):
    out = m.heap.alloc(144)
    assert call(m, "xgameruntime.dll", "XSystemGetAnalyticsInfo", out) == out
    assert bytes(m.cpu.mem_read(out + 16 + 64, 13)) == b"Xbox Series X"


def test_missing_function_returns_e_notimpl(m):
    assert m._missing("d3d12_xs.dll!D3D12CreateDevice") == E_NOTIMPL
    m._missing("d3d12_xs.dll!D3D12CreateDevice")
    assert m.missing_calls == {"d3d12_xs.dll!D3D12CreateDevice": 2}
    assert len(m.logs) == 1


def test_report_text_lists_missing():
    report = Report("jeu.exe", 2, ("a!b",), ("c!d",), True, False)
    text = report.text()
    assert "50.0 %" in text and "c!d" in text and "TLS: yes" in text


def test_heap_limit_returns_null(m):
    assert m.heap.alloc(1 << 40) == 0


def test_onexit_table_is_cleared(m):
    table = m.heap.alloc(24)
    m.cpu.mem_write(table, b"\xff" * 24)
    assert call(m, "api-ms-win-crt-runtime-l1-1-0.dll", "_initialize_onexit_table", table) == 0
    assert bytes(m.cpu.mem_read(table, 24)) == bytes(24)


def test_processor_features_match_unicorn(m):
    assert call(m, "kernel32.dll", "IsProcessorFeaturePresent", 38) == 1  # SSE4.2
    assert call(m, "kernel32.dll", "IsProcessorFeaturePresent", 40) == 0  # AVX2 non émulé


def test_system_info_describes_amd64(m):
    info = m.heap.alloc(48)
    call(m, "kernel32.dll", "GetNativeSystemInfo", info)
    arch, _, page = struct.unpack("<HHI", m.cpu.mem_read(info, 8))
    assert (arch, page) == (9, 0x1000)


def test_environment_is_empty(m):
    assert call(m, "kernelx.dll", "GetEnvironmentVariableW", 0, 0, 0) == 0
    assert m.last_error == 203


def test_math_uses_xmm_registers(m):
    from unicorn.x86_const import UC_X86_REG_XMM0, UC_X86_REG_XMM1
    def pack(v):
        return struct.unpack("<Q", struct.pack("<d", v))[0]
    m.cpu.reg_write(UC_X86_REG_XMM0, pack(2.0))
    m.cpu.reg_write(UC_X86_REG_XMM1, pack(10.0))
    hle.resolve("ucrtbase.dll", "pow")[1](hle.Call(m))
    result = m.cpu.reg_read(UC_X86_REG_XMM0) & 0xFFFF_FFFF_FFFF_FFFF
    assert struct.unpack("<d", struct.pack("<Q", result))[0] == 1024.0


def test_setlocale_returns_c_string(m):
    pointer = call(m, "ucrtbase.dll", "setlocale", 0, 0)
    assert bytes(m.cpu.mem_read(pointer, 2)) == b"C\0"


def test_ctype_table_classifies_ascii(m):
    table = call(m, "ucrtbase.dll", "__pctype_func")
    def flags(ch):
        return struct.unpack("<H", m.cpu.mem_read(table + 2 * ord(ch), 2))[0]
    assert flags("A") & 0x1 and flags("a") & 0x2 and flags("7") & 0x4 and flags(" ") & 0x8
    assert struct.unpack("<H", m.cpu.mem_read(table - 2, 2))[0] == 0  # EOF


def test_bench_on_demo_folder():
    from emu import bench
    rows = bench.run(DEMO.parent)
    assert rows == ()  # demo.pe n'a pas l'extension .dll/.exe
    row = bench.run_one(DEMO)
    assert row.loaded and row.imports == 1
    assert "Initialisation réussie" in bench.table((row,))
