"""xmem.dll en mode isolé : allocateur mémoire de la console sur le tas émulé.

Même décodage des attributs XALLOC_ATTRIBUTES (disposition ERA) et même règle d'alignement
que le runtime C++ (runtime/xmem/era_heap.cpp) : tas et types graphiques servis, mémoire
physique et champs non définis refusés.
"""
from .core import hle

SMALL_ALIGNMENT = 64
PAGE_ALIGNMENT = 4096
GRANULARITY = 64 * 1024                       # début d'une région VirtualAlloc sur PC x64
PAGE_BYTES = (4 * 1024, 64 * 1024, 4 * 1024 * 1024)
MIN_ALIGNMENT_LOG2 = 4
GRAPHICS_LAST, PHYSICAL_LAST, UNDEFINED_PAGE = 6, 9, 3
ERROR_NOT_SUPPORTED = 50
ERROR_INVALID_PARAMETER = 87


def decode(attributes):
    """Champs ERA : (objectType, pageSize, allocatorId, log2 alignement, memoryType, réservé)."""
    return (attributes & 0x3FFF, (attributes >> 14) & 3, (attributes >> 16) & 0xFF,
            (attributes >> 24) & 0x1F, (attributes >> 29) & 0xF, attributes >> 33)


def placement(size, attributes):
    """Alignement du bloc, ou code d'erreur Windows (négatif) si les attributs sont refusés."""
    _, page_size, _, alignment_log2, memory_type, reserved = decode(attributes)
    if reserved or page_size == UNDEFINED_PAGE or memory_type > PHYSICAL_LAST:
        return -ERROR_INVALID_PARAMETER
    if memory_type > GRAPHICS_LAST:
        return -ERROR_NOT_SUPPORTED
    alignment = 1 << max(MIN_ALIGNMENT_LOG2, alignment_log2)
    if memory_type == 0:
        return max(alignment, PAGE_ALIGNMENT if size >= PAGE_ALIGNMENT else SMALL_ALIGNMENT)
    if max(PAGE_BYTES[page_size], alignment) > GRANULARITY:
        return -ERROR_NOT_SUPPORTED
    return GRANULARITY


@hle("xmem", "XMemAlloc", "XMemAllocDefault")
def xmem_alloc(c):
    size = c.arg(0) or 1
    alignment = placement(size, c.arg(1))
    if alignment < 0:
        c.machine.last_error = -alignment
        return 0
    return c.heap.alloc(size, alignment)


@hle("xmem", "XMemFree", "XMemFreeDefault")
def xmem_free(c):
    if c.arg(0):
        c.heap.free(c.arg(0))


@hle("xmem")
def XMemGetWorkingSetStatistics(c):
    """Structure non publique : échec signalé, comme dans le runtime C++."""
    c.machine.last_error = ERROR_NOT_SUPPORTED
    return 0
