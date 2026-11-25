"""xmem.dll C++ : attributs d'allocation XALLOC_ATTRIBUTES (disposition ERA) et XMemVirtualQuery.

Les deux mots d'attributs sont ceux que Xbox2Windows a relevés aux appels de Bink et d'AMD :
0x05e50000 (tas, alignement 32) et 0xa8004000 (type graphique 5, pages de 64 Kio, alignement 256).
"""
import ctypes
import sys

import pytest

pytestmark = pytest.mark.skipif(sys.platform != "win32", reason="DLL Windows x64")

BINK_WORD = 0x05E5_0000
AMD_WORD = 0xA800_4000
MEM_COMMIT, MEM_FREE, PAGE_READWRITE = 0x1000, 0x10000, 0x04
ERROR_INVALID_PARAMETER, ERROR_NOT_SUPPORTED, ERROR_NOACCESS = 87, 50, 998
GRANULARITY = 64 * 1024
TAIL = 0x30
KERNEL_ADDRESS = 0xFFFF_8000_0000_0000


def era_word(memory_type=0, alignment_log2=0, page_size=0, allocator=0, object_type=0, reserved=0):
    return (object_type | page_size << 14 | allocator << 16 | alignment_log2 << 24
            | memory_type << 29 | reserved << 33)


class Fields(ctypes.Structure):
    _fields_ = [("object_type", ctypes.c_uint32), ("page_size", ctypes.c_uint32),
                ("allocator_id", ctypes.c_uint32), ("alignment", ctypes.c_uint32),
                ("memory_type", ctypes.c_uint32), ("reserved", ctypes.c_uint64)]


class MemoryInfo(ctypes.Structure):
    _fields_ = [("base", ctypes.c_void_p), ("allocation_base", ctypes.c_void_p),
                ("allocation_protect", ctypes.c_uint32), ("partition", ctypes.c_uint16),
                ("region_size", ctypes.c_size_t), ("state", ctypes.c_uint32),
                ("protect", ctypes.c_uint32), ("type", ctypes.c_uint32)]


class QueryBuffer(ctypes.Structure):
    """Tampon de 64 octets des appelants BVH : MEMORY_BASIC_INFORMATION puis l'octet +0x30."""
    _fields_ = [("info", MemoryInfo), ("tail", ctypes.c_ubyte * 16)]


@pytest.fixture
def xmem(runtime_dir):
    dll = ctypes.WinDLL(str(runtime_dir / "xmem.dll"), use_last_error=True)
    dll.XMemAlloc.restype = ctypes.c_void_p
    dll.XMemAlloc.argtypes = (ctypes.c_size_t, ctypes.c_uint64)
    dll.XMemFree.argtypes = (ctypes.c_void_p, ctypes.c_uint64)
    dll.XMemVirtualQuery.restype = ctypes.c_int32
    dll.XMemVirtualQuery.argtypes = (ctypes.c_void_p, ctypes.c_void_p, ctypes.c_void_p)
    dll.AnyXboxX_DecodeAllocAttributes.argtypes = (ctypes.c_uint64, ctypes.POINTER(Fields))
    return dll


def windows_query(address):
    kernel32 = ctypes.WinDLL("kernel32")
    kernel32.VirtualQuery.argtypes = (ctypes.c_void_p, ctypes.c_void_p, ctypes.c_size_t)
    kernel32.VirtualQuery.restype = ctypes.c_size_t
    info = MemoryInfo()
    assert kernel32.VirtualQuery(address, ctypes.byref(info), ctypes.sizeof(info)) == ctypes.sizeof(info)
    return info


def query(xmem, address, process=ctypes.c_void_p(-1)):
    buffer = QueryBuffer()
    for index in range(1, 16):
        buffer.tail[index] = 0xA5                 # sentinelle : seul l'octet +0x30 est écrit
    return xmem.XMemVirtualQuery(process, address, ctypes.byref(buffer)), buffer


def test_attribute_words_split_with_the_era_layout(xmem):
    fields = Fields()
    xmem.AnyXboxX_DecodeAllocAttributes(BINK_WORD, ctypes.byref(fields))
    assert (fields.memory_type, fields.page_size, fields.allocator_id, fields.alignment) == (0, 0, 0xE5, 5)
    xmem.AnyXboxX_DecodeAllocAttributes(AMD_WORD, ctypes.byref(fields))
    assert (fields.memory_type, fields.page_size, fields.allocator_id, fields.alignment) == (5, 1, 0, 8)
    xmem.AnyXboxX_DecodeAllocAttributes(era_word(object_type=0x1234, reserved=3), ctypes.byref(fields))
    assert (fields.object_type, fields.reserved) == (0x1234, 3)


@pytest.mark.parametrize("alignment_log2", [5, 12, 16, 20])
def test_heap_request_honours_the_requested_alignment(xmem, alignment_log2):
    word = era_word(alignment_log2=alignment_log2, allocator=0xE5)
    block = xmem.XMemAlloc(257, word)
    try:
        assert block and block % (1 << alignment_log2) == 0 and block % 64 == 0
        assert ctypes.string_at(block, 257) == bytes(257)          # rendu à zéro
        ctypes.memset(block, 0x51, 257)
    finally:
        xmem.XMemFree(block, word)


def test_graphics_memory_gets_its_own_committed_region(xmem):
    block = xmem.XMemAlloc(512, AMD_WORD)
    assert block and block % GRANULARITY == 0
    info = windows_query(block)
    assert (info.allocation_base, info.state, info.protect) == (block, MEM_COMMIT, PAGE_READWRITE)
    assert ctypes.string_at(block, 512) == bytes(512)
    ctypes.memset(block, 0x51, 512)
    xmem.XMemFree(block, AMD_WORD)
    assert windows_query(block).state == MEM_FREE


def test_physical_memory_request_of_the_title_is_served(xmem):
    # XMemAlloc(0x60000, 0xf0814000) du titre : type physique 7, pages de 64 Kio, alignement 64 Kio.
    # Un NULL ici fait prendre au titre sa branche d'erreur.
    word = 0xF081_4000
    block = xmem.XMemAlloc(0x6_0000, word)
    try:
        assert block and block % (64 * 1024) == 0
        assert ctypes.string_at(block, 0x6_0000) == bytes(0x6_0000)
        ctypes.memset(block, 0x51, 0x6_0000)
    finally:
        xmem.XMemFree(block, word)


@pytest.mark.parametrize("word, error", [
    (era_word(memory_type=7, alignment_log2=17), ERROR_NOT_SUPPORTED),   # physique, alignement > 64 Kio
    (era_word(memory_type=9, alignment_log2=20), ERROR_NOT_SUPPORTED),
    (era_word(memory_type=12), ERROR_INVALID_PARAMETER),          # type non défini
    (era_word(page_size=3), ERROR_INVALID_PARAMETER),             # taille de page non définie
    (BINK_WORD | 1 << 40, ERROR_INVALID_PARAMETER),               # bit réservé
    (era_word(memory_type=5, page_size=2), ERROR_NOT_SUPPORTED),  # pages de 4 Mio graphiques
    (era_word(memory_type=5, alignment_log2=17), ERROR_NOT_SUPPORTED),
])
def test_unservable_attributes_return_null_with_an_error(xmem, word, error):
    ctypes.set_last_error(0)
    assert xmem.XMemAlloc(4096, word) is None
    assert ctypes.get_last_error() == error


def test_virtual_query_flags_graphics_memory_only(xmem):
    graphics, heap = xmem.XMemAlloc(4096, AMD_WORD), xmem.XMemAlloc(257, BINK_WORD)
    try:
        ok, buffer = query(xmem, graphics + 100)
        assert ok == 1 and buffer.info.state == MEM_COMMIT and buffer.info.protect == PAGE_READWRITE
        assert buffer.info.allocation_base == graphics
        assert buffer.tail[0] == 1 and list(buffer.tail[1:]) == [0xA5] * 15
        ok, buffer = query(xmem, heap)
        assert ok == 1 and buffer.info.state == MEM_COMMIT and buffer.tail[0] == 0
    finally:
        xmem.XMemFree(graphics, AMD_WORD)
        xmem.XMemFree(heap, BINK_WORD)
    ok, buffer = query(xmem, graphics)
    assert ok == 1 and buffer.info.state == MEM_FREE and buffer.tail[0] == 0


def test_virtual_query_accepts_a_real_process_handle(xmem):
    kernel32 = ctypes.WinDLL("kernel32")
    kernel32.OpenProcess.restype = ctypes.c_void_p
    kernel32.CloseHandle.argtypes = (ctypes.c_void_p,)
    handle = kernel32.OpenProcess(0x0400, False, kernel32.GetCurrentProcessId())   # QUERY_INFORMATION
    graphics = xmem.XMemAlloc(4096, AMD_WORD)
    try:
        ok, buffer = query(xmem, graphics, ctypes.c_void_p(handle))
        assert ok == 1 and buffer.tail[0] == 1
    finally:
        xmem.XMemFree(graphics, AMD_WORD)
        kernel32.CloseHandle(handle)


def test_virtual_query_fails_cleanly(xmem):
    buffer = QueryBuffer()
    ctypes.memset(ctypes.byref(buffer), 0xD7, ctypes.sizeof(buffer))
    assert xmem.XMemVirtualQuery(ctypes.c_void_p(-1), KERNEL_ADDRESS, ctypes.byref(buffer)) == 0
    assert ctypes.string_at(ctypes.byref(buffer), ctypes.sizeof(buffer)) == b"\xD7" * 64
    ctypes.set_last_error(0)
    assert xmem.XMemVirtualQuery(ctypes.c_void_p(-1), ctypes.addressof(buffer), None) == 0
    assert ctypes.get_last_error() == ERROR_NOACCESS


def test_free_follows_the_block_even_with_other_attributes(xmem):
    """Le bloc est rendu à l'allocateur qui l'a produit, quels que soient les attributs passés."""
    block = xmem.XMemAlloc(512, AMD_WORD)
    xmem.XMemFree(block, 0)
    assert windows_query(block).state == MEM_FREE
    ok, buffer = query(xmem, block)
    assert ok == 1 and buffer.tail[0] == 0
