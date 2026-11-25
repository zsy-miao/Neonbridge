"""Régressions issues de la relecture du code."""
import struct

import pefile
import pytest

from emu import hle
from emu.config import read_package
from emu.loader import read_pe
from emu.machine import Machine
from emu.modules import MODULE_AREA
from scarlett import DEMO


def rebased_demo(tmp_path, with_relocations=True):
    """Copie de demo.pe dont la base chevauche la zone réservée de l'émulateur."""
    pe = pefile.PE(str(DEMO))
    pe.OPTIONAL_HEADER.ImageBase = 0x7000_0000_0000
    if with_relocations:
        pdata = next(s for s in pe.sections if s.Name.startswith(b".pdata"))
        # Un bloc de relocation vide (8 octets) suffit : la démo n'a pas d'adresse absolue.
        pe.set_bytes_at_rva(pdata.VirtualAddress, struct.pack("<II", 0x1000, 8))
        pe.OPTIONAL_HEADER.DATA_DIRECTORY[5].VirtualAddress = pdata.VirtualAddress
        pe.OPTIONAL_HEADER.DATA_DIRECTORY[5].Size = 8
    path = tmp_path / "rebased.pe"
    pe.write(str(path))
    return path


def test_rebased_image_runs(tmp_path):
    machine = Machine(rebased_demo(tmp_path))
    base = machine.image.base
    assert base == MODULE_AREA and machine.image.relocated
    slot = machine.image.imports[0].slot
    assert base <= slot < base + machine.image.size
    assert machine.start() == 7


def test_rebase_without_relocations_is_refused(tmp_path):
    with pytest.raises(ValueError, match="relocation"):
        read_pe(rebased_demo(tmp_path, with_relocations=False))


def test_strlen_counts_raw_bytes():
    m = Machine(DEMO)
    text = m.heap.alloc(6000)
    m.cpu.mem_write(text, b"\xff" * 5000 + b"\0")
    m.cpu.reg_write(hle.core.ARG_REGISTERS[0], text)
    assert hle.resolve("ucrtbase.dll", "strlen")[1](hle.Call(m)) == 5000


def test_wcslen_counts_utf16_units():
    m = Machine(DEMO)
    text = m.heap.alloc(16)
    m.cpu.mem_write(text, "a😀".encode("utf-16-le") + b"\0\0")
    m.cpu.reg_write(hle.core.ARG_REGISTERS[0], text)
    assert hle.resolve("ucrtbase.dll", "wcslen")[1](hle.Call(m)) == 3


def test_exit_blocks_later_calls():
    m = Machine(DEMO)
    m.request_exit(0)
    with pytest.raises(ValueError, match="terminé"):
        m.call(m.image.entry)


def test_virtualalloc_commit_inside_reservation():
    m = Machine(DEMO)
    regs = hle.core.ARG_REGISTERS

    def virtual_alloc(address, size):
        m.cpu.reg_write(regs[0], address)
        m.cpu.reg_write(regs[1], size)
        return hle.resolve("kernel32.dll", "VirtualAlloc")[1](hle.Call(m))

    base = virtual_alloc(0, 0x40000)
    assert virtual_alloc(base + 0x10000, 0x1000) == base + 0x10000
    assert virtual_alloc(0, 0) == 0


def test_utf16_manifest_with_entities_is_refused(tmp_path):
    xml = '<!DOCTYPE g [<!ENTITY a "x">]><Game/>'
    (tmp_path / "MicrosoftGame.Config").write_bytes(xml.encode("utf-16"))
    with pytest.raises(ValueError, match="entités"):
        read_package(tmp_path)


def test_handler_crash_becomes_value_error():
    m = Machine(DEMO)
    address = next(iter(m.handlers))
    name, _ = m.handlers[address]
    m.handlers[address] = (name, lambda c: 1 / 0)
    with pytest.raises(ValueError, match="division"):
        m.start()


def test_strlen_stops_at_end_of_mapping():
    m = Machine(DEMO)
    from emu.memory import PAGE
    end = m.heap.mapped
    text = end - 3
    m.cpu.mem_write(text, b"ab\0")
    m.cpu.reg_write(hle.core.ARG_REGISTERS[0], text)
    assert end % PAGE == 0
    assert hle.resolve("ucrtbase.dll", "strlen")[1](hle.Call(m)) == 2
