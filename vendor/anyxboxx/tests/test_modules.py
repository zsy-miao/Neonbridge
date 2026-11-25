"""Chargement de plusieurs modules PE liés dans une même machine."""
import struct

import pytest

from emu import bench, hle
from emu.machine import Machine
from emu.report import analyse
from pe_builder import DLL_BASE, Asm, build_pe, rva

LEA_EAX_RCX_PLUS_1 = bytes.fromhex("8d4101")
LEA_EAX_RCX_TIMES_2 = bytes.fromhex("8d0409")
FLAG = 0x100        # dword écrit par DllMain de libB
POINTER = 0x80      # qword absolu de libB, corrigé par relocation


def lib_b(folder):
    code = (Asm().store_dword(FLAG, 0x55).mov_eax(1).ret()
            .at(0x10).raw(LEA_EAX_RCX_TIMES_2).ret()
            .at(0x20).mov_rax_from(POINTER).ret()
            .build())
    return build_pe(folder / "libB.dll", lambda iat: code,
                    exports=[("Double", 0x10), ("DoublePtr", 0x20)], pointers={POINTER: 0x10})


def lib_a(folder, dll_main_result=1):
    code = Asm().mov_eax(dll_main_result).ret().at(0x10).raw(LEA_EAX_RCX_PLUS_1).ret().build()
    return build_pe(folder / "libA.dll", lambda iat: code,
                    exports=[("Add1", 0x10), ("Twice", "libB.Double")])


def game(folder, extra_imports=()):
    imports = [("libA.dll", "Add1"), ("libA.dll", "Twice"), ("libB.dll", 1), *extra_imports]

    def code(iat):
        return (Asm().frame().mov_ecx(20)
                .call_iat(iat[("libA.dll", "Add1")]).mov_ecx_eax()
                .call_iat(iat[("libA.dll", "Twice")]).mov_ecx_eax()
                .call_iat(iat[("libB.dll", 1)])
                .unframe().ret().build())

    return build_pe(folder / "game.exe", code, imports=imports, dll=False)


@pytest.fixture
def package(tmp_path):
    lib_a(tmp_path)
    lib_b(tmp_path)
    return game(tmp_path)


def test_main_runs_through_linked_modules(package):
    # ((20 + 1) × 2) × 2 : Add1 dans libA, Twice redirigé vers libB, Double par ordinal.
    assert Machine(package).start() == 84


def test_dependencies_are_initialised_before_main(package):
    machine = Machine(package)
    machine.start()
    order = [m.name for m in machine.modules.init_order(machine.main)]
    assert order[-1] == "game.exe" and set(order) == {"liba.dll", "libb.dll", "game.exe"}
    lib = machine.modules.get("libB")
    assert struct.unpack("<I", machine.cpu.mem_read(lib.image.base + rva(FLAG), 4))[0] == 0x55


def test_colliding_module_is_relocated(package):
    machine = Machine(package)
    lib_a_base = machine.modules.get("libA.dll").image.base
    lib = machine.modules.get("libB.dll")
    assert lib_a_base == DLL_BASE and lib.image.base != DLL_BASE
    pointer = machine.call(lib.image.exports["DoublePtr"])
    assert pointer == lib.image.base + rva(0x10)


def call(machine, dll, name, *args):
    for register, value in zip(hle.core.ARG_REGISTERS, args, strict=False):
        machine.cpu.reg_write(register, value)
    return hle.resolve(dll, name)[1](hle.Call(machine))


def text(machine, value, wide=True):
    data = value.encode("utf-16-le") + b"\0\0" if wide else value.encode() + b"\0"
    address = machine.heap.alloc(len(data))
    machine.cpu.mem_write(address, data)
    return address


def test_module_handle_and_proc_address(package):
    machine = Machine(package)
    lib = machine.modules.get("libB.dll")
    handle = call(machine, "kernel32.dll", "GetModuleHandleW", text(machine, "LIBB"))
    assert handle == lib.image.base
    proc = call(machine, "kernel32.dll", "GetProcAddress", handle, text(machine, "Double", False))
    assert proc == lib.image.base + rva(0x10)
    assert call(machine, "kernel32.dll", "GetProcAddress", handle, 1) == proc  # ordinal


def test_proc_address_of_emulated_system_function(package):
    machine = Machine(package)
    kernel = call(machine, "kernel32.dll", "GetModuleHandleW", text(machine, "kernel32.dll"))
    assert kernel
    tick = call(machine, "kernel32.dll", "GetProcAddress", kernel, text(machine, "GetTickCount64", False))
    assert tick in machine.handlers
    missing = call(machine, "kernel32.dll", "GetProcAddress", kernel, text(machine, "Nope", False))
    assert missing == 0 and machine.last_error == 127


def test_pc_to_file_header(package):
    machine = Machine(package)
    lib = machine.modules.get("libA.dll")
    out = machine.heap.alloc(8)
    assert call(machine, "ntdll.dll", "RtlPcToFileHeader", lib.image.base + 0x1010, out) == lib.image.base


def test_unknown_dll_falls_back_to_hle(tmp_path):
    lib_a(tmp_path)
    lib_b(tmp_path)
    machine = Machine(game(tmp_path, extra_imports=[("absent.dll", "Foo")]))
    assert machine.statuses["absent.dll!Foo"] == "absent"
    assert machine.start() == 84


def test_system_dll_is_never_loaded_from_package(tmp_path):
    lib_a(tmp_path)
    lib_b(tmp_path)
    lib_a(tmp_path).rename(tmp_path / "kernel32.dll")
    lib_a(tmp_path)
    machine = Machine(game(tmp_path, extra_imports=[("KERNEL32.dll", "GetTickCount64")]))
    assert machine.statuses["KERNEL32.dll!GetTickCount64"] == "python"
    assert machine.modules.get("kernel32.dll") is None


def test_failed_dll_main_stops_start(tmp_path):
    lib_a(tmp_path, dll_main_result=0)
    lib_b(tmp_path)
    with pytest.raises(ValueError, match="DllMain"):
        Machine(game(tmp_path)).start()


def test_report_counts_imports_from_package(package):
    report = analyse(package)
    assert len(report.provided) == 3 and report.coverage == 100.0


def test_bench_initialises_every_module(package):
    rows = bench.run(package.parent)
    assert [r.name for r in rows] == ["game.exe", "libA.dll", "libB.dll"]
    assert all(r.initialised for r in rows)


def test_etw_registration_and_mb_cur_max(package):
    machine = Machine(package)
    assert call(machine, "api-ms-win-crt-locale-l1-1-0.dll", "___mb_cur_max_func") == 1
    kind, handler = hle.resolve("api-ms-win-eventing-classicprovider-l1-1-0.dll", "RegisterTraceGuidsW")
    assert kind == "python"
