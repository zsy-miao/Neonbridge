"""xmem.dll C++ : XMemVirtualAlloc et les appels de mémoire physique, aux formes mesurées au premier démarrage.

Formes relevées à l'exécution du titre (docs/research/first-boot-1021.md, section 4) : XMemVirtualAlloc(adresse,
taille, type d'allocation, attributs, protection) avec des types 0x20401000, 0x20402000, 0x20406000, 0x20003000 et
0x42000 ; XMemAllocatePhysicalPages(0x280 ou 0x380, compte, tableau de numéros de page). Aucun fichier du jeu :
les formes sont des constantes, les appels passent par la DLL compilée.
"""
import ctypes
import sys

import pytest

pytestmark = pytest.mark.skipif(sys.platform != "win32", reason="DLL Windows x64")

MEM_COMMIT, MEM_RESERVE = 0x1000, 0x2000
PAGE_NOACCESS = 0x01
ERROR_INVALID_PARAMETER, ERROR_NOT_SUPPORTED = 87, 50
GRANULARITY = 64 * 1024
PHYSICAL_PAGE = 64 * 1024
MEASURED_SEVEN = ("XMemVirtualAlloc", "XMemAllocatePhysicalPages", "XMemFreePhysicalPages",
                  "XMemMapPhysicalPages", "XMemMapPhysicalPagesScatter", "XMemIsToolsMemAvailable",
                  "XMemTransferMemory")


class MemoryInfo(ctypes.Structure):
    _fields_ = [("base", ctypes.c_void_p), ("allocation_base", ctypes.c_void_p),
                ("allocation_protect", ctypes.c_uint32), ("partition", ctypes.c_uint16),
                ("region_size", ctypes.c_size_t), ("state", ctypes.c_uint32),
                ("protect", ctypes.c_uint32), ("type", ctypes.c_uint32)]


# Signatures des exports testés. Un export absent reste absent : l'accès à l'attribut échoue dans le test.
SIGNATURES = {
    "XMemVirtualAlloc": (ctypes.c_void_p, (ctypes.c_void_p, ctypes.c_size_t, ctypes.c_uint32, ctypes.c_uint64,
                                           ctypes.c_uint32)),
    "XMemAllocatePhysicalPages": (ctypes.c_int, (ctypes.c_uint64, ctypes.c_void_p, ctypes.c_void_p)),
    "XMemFreePhysicalPages": (ctypes.c_int, (ctypes.c_size_t, ctypes.c_void_p)),
    "XMemMapPhysicalPages": (ctypes.c_void_p, (ctypes.c_void_p, ctypes.c_size_t, ctypes.c_void_p)),
    "XMemMapPhysicalPagesScatter": (ctypes.c_void_p, (ctypes.c_void_p, ctypes.c_size_t, ctypes.c_void_p)),
    "XMemIsToolsMemAvailable": (ctypes.c_int, ()),
    "XMemTransferMemory": (ctypes.c_int, ()),
}


def declared(dll, name):
    """Fonction de la DLL avec son type de retour et ses arguments ; absente : AttributeError."""
    function = getattr(dll, name)
    function.restype, function.argtypes = SIGNATURES[name]
    return function


@pytest.fixture
def xmem(runtime_dir):
    dll = ctypes.WinDLL(str(runtime_dir / "xmem.dll"), use_last_error=True)
    return Exports(dll)


class Exports:
    """Accès nommé aux exports : xmem.XMemVirtualAlloc(...) déclare la signature au premier appel."""

    def __init__(self, dll):
        self._dll = dll

    def __getattr__(self, name):
        return declared(self._dll, name)


def windows_query(address):
    kernel32 = ctypes.WinDLL("kernel32")
    kernel32.VirtualQuery.argtypes = (ctypes.c_void_p, ctypes.c_void_p, ctypes.c_size_t)
    kernel32.VirtualQuery.restype = ctypes.c_size_t
    info = MemoryInfo()
    assert kernel32.VirtualQuery(address, ctypes.addressof(info), ctypes.sizeof(info)) == ctypes.sizeof(info)
    return info


def release(address):
    """Libère une réservation ou un bloc VirtualAlloc, par son adresse de base."""
    kernel32 = ctypes.WinDLL("kernel32")
    kernel32.VirtualFree.argtypes = (ctypes.c_void_p, ctypes.c_size_t, ctypes.c_uint32)
    kernel32.VirtualFree(address, 0, 0x8000)   # MEM_RELEASE


# Formes mesurées sur une copie possédée légalement, voir docs/research/first-boot-1021.md
# (measured on a lawfully owned copy) : (type d'allocation, attributs, protection, état attendu).
MEASURED_SHAPES = [
    (0x20401000, 0x283, 0x42404, MEM_COMMIT),                 # commit seul, sans réservation
    (0x20402000, 0x21, 4, MEM_RESERVE),                       # jamais NULL, réservation seule
    (0x20406000, 0x27, 0x1004, MEM_RESERVE),                  # pages de 64 Kio, protection 0x1004
    (0x20003000, 0x183, 0x22004, MEM_COMMIT),                 # réservation et commit
    (0x42000, 7, 1, MEM_RESERVE),                             # tas, protection PAGE_NOACCESS
]


@pytest.mark.parametrize("alloc_type, attributes, protect, state", MEASURED_SHAPES)
def test_measured_shapes_return_an_address_in_the_expected_state(xmem, alloc_type, attributes, protect, state):
    size = 0x10000 if alloc_type != 0x20003000 else 0x400_0000
    address = xmem.XMemVirtualAlloc(None, size, alloc_type, attributes, protect)
    try:
        assert address, "XMemVirtualAlloc a rendu NULL pour une forme mesurée"
        assert windows_query(address).state == state
    finally:
        release(address)


def test_reserved_block_is_not_null_for_0x20402000_even_with_a_big_size(xmem):
    address = xmem.XMemVirtualAlloc(None, 0x4000_0000, 0x20402000, 0x21, 4)
    try:
        assert address
    finally:
        release(address)


def test_reserved_regions_are_granularity_aligned(xmem):
    address = xmem.XMemVirtualAlloc(None, 0x10000, 0x20406000, 0x27, 0x1004)
    try:
        assert address % GRANULARITY == 0
        info = windows_query(address)
        assert info.allocation_base == address
    finally:
        release(address)


def test_commit_into_a_reservation_makes_it_writable(xmem):
    reserved = xmem.XMemVirtualAlloc(None, 0x10000, 0x20402000, 0x21, 4)
    try:
        committed = xmem.XMemVirtualAlloc(reserved, 0x10000, 0x20401000, 0x283, 4)
        assert committed == reserved
        assert windows_query(reserved).state == MEM_COMMIT
        ctypes.memset(reserved, 0x51, 0x10000)
    finally:
        release(reserved)


def test_undefined_protection_is_refused_with_an_error(xmem):
    ctypes.set_last_error(0)
    assert xmem.XMemVirtualAlloc(None, 0x10000, 0x42000, 7, 0) is None
    assert ctypes.get_last_error() == ERROR_INVALID_PARAMETER


def test_allocation_type_without_reserve_or_commit_is_refused(xmem):
    ctypes.set_last_error(0)
    assert xmem.XMemVirtualAlloc(None, 0x10000, 0x20400000, 0x21, 4) is None
    assert ctypes.get_last_error() == ERROR_INVALID_PARAMETER


def test_physical_pages_are_allocated_then_mapped_and_freed(xmem):
    count = ctypes.c_size_t(2)
    pfns = (ctypes.c_size_t * 2)()
    assert xmem.XMemAllocatePhysicalPages(0x280, ctypes.addressof(count), ctypes.addressof(pfns)) == 1
    assert count.value == 2 and pfns[0] and pfns[1] and pfns[0] != pfns[1]

    reserved = xmem.XMemVirtualAlloc(None, 2 * PHYSICAL_PAGE, 0x20402000, 0x21, 4)
    try:
        mapped = xmem.XMemMapPhysicalPages(reserved, 2, ctypes.addressof(pfns))
        assert mapped == reserved
        assert windows_query(reserved).state == MEM_COMMIT
        ctypes.memset(reserved, 0x51, 2 * PHYSICAL_PAGE)
    finally:
        release(reserved)
    assert xmem.XMemFreePhysicalPages(2, ctypes.addressof(pfns)) == 1


def test_physical_pages_accept_the_0x380_attribute_word(xmem):
    count = ctypes.c_size_t(1)
    pfns = (ctypes.c_size_t * 1)()
    assert xmem.XMemAllocatePhysicalPages(0x380, ctypes.addressof(count), ctypes.addressof(pfns)) == 1
    assert count.value == 1 and pfns[0]
    assert xmem.XMemFreePhysicalPages(1, ctypes.addressof(pfns)) == 1


def test_physical_allocation_of_zero_pages_is_refused(xmem):
    count = ctypes.c_size_t(0)
    pfns = (ctypes.c_size_t * 1)()
    ctypes.set_last_error(0)
    assert xmem.XMemAllocatePhysicalPages(0x280, ctypes.addressof(count), ctypes.addressof(pfns)) == 0
    assert ctypes.get_last_error() == ERROR_INVALID_PARAMETER


def test_mapping_an_unknown_page_number_returns_null(xmem):
    pfns = (ctypes.c_size_t * 1)(0x1234_5678)
    ctypes.set_last_error(0)
    assert xmem.XMemMapPhysicalPages(None, 1, ctypes.addressof(pfns)) is None
    assert ctypes.get_last_error() == ERROR_INVALID_PARAMETER


def test_scatter_mapping_maps_each_page_like_the_plain_call(xmem):
    count = ctypes.c_size_t(3)
    pfns = (ctypes.c_size_t * 3)()
    assert xmem.XMemAllocatePhysicalPages(0x280, ctypes.addressof(count), ctypes.addressof(pfns)) == 1
    mapped = xmem.XMemMapPhysicalPagesScatter(None, 3, ctypes.addressof(pfns))
    try:
        assert mapped
        ctypes.memset(mapped, 0x51, 3 * PHYSICAL_PAGE)
    finally:
        release(mapped)
    assert xmem.XMemFreePhysicalPages(3, ctypes.addressof(pfns)) == 1


def test_tools_memory_is_reported_absent_on_a_retail_console(xmem):
    assert xmem.XMemIsToolsMemAvailable() == 0


def test_transfer_memory_is_not_served_and_says_so(xmem):
    ctypes.set_last_error(0)
    assert xmem.XMemTransferMemory() == 0
    assert ctypes.get_last_error() == ERROR_NOT_SUPPORTED


@pytest.mark.parametrize("name", MEASURED_SEVEN)
def test_the_seven_imports_the_title_expects_are_exported(runtime_dir, name):
    dll = ctypes.WinDLL(str(runtime_dir / "xmem.dll"))
    assert getattr(dll, name) is not None
