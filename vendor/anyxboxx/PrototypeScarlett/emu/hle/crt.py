"""Fonctions courantes du runtime C (ucrtbase, vcruntime140, api-ms-win-crt-*)."""
import struct

from .core import hle

MAX_COPY = 64 * 1024 * 1024
FILE_SIZE = 0x58  # taille d'une structure FILE de l'UCRT x64
LC_CATEGORIES = 6
STRING_CHUNK = 4096
ABORT_CODE = 3
# Drapeaux de <ctype.h> (_UPPER, _LOWER, _DIGIT, _SPACE, _PUNCT, _CONTROL, _BLANK, _HEX, _ALPHA).
CT_UPPER, CT_LOWER, CT_DIGIT, CT_SPACE, CT_PUNCT = 0x1, 0x2, 0x4, 0x8, 0x10
CT_CONTROL, CT_BLANK, CT_HEX, CT_ALPHA = 0x20, 0x40, 0x80, 0x100


def _bounded(size):
    if size > MAX_COPY:
        raise ValueError(f"Copie mémoire de {size} octets refusée par le prototype.")
    return size


@hle("crt", "malloc", "_malloc_base")
def malloc(c):
    return c.heap.alloc(c.arg(0))


@hle("crt", "calloc", "_calloc_base")
def calloc(c):
    return c.heap.alloc(c.arg(0) * c.arg(1))  # le tas neuf est déjà à zéro


@hle("crt", "realloc", "_realloc_base")
def realloc(c):
    return c.heap.realloc(c.arg(0), c.arg(1))


@hle("crt", "free", "_free_base")
def free(c):
    c.heap.free(c.arg(0))


@hle("crt", "_aligned_malloc")
def aligned_malloc(c):
    return c.heap.alloc(c.arg(0), alignment=max(c.arg(1), 16))


@hle("crt", "_aligned_free")
def aligned_free(c):
    c.heap.free(c.arg(0))


@hle("crt")
def memset(c):
    dest, value, size = c.arg(0), c.arg(1) & 0xFF, _bounded(c.arg(2))
    if size:
        c.write(dest, bytes([value]) * size)
    return dest


@hle("crt", "memcpy", "memmove")
def memcpy(c):
    dest, src, size = c.arg(0), c.arg(1), _bounded(c.arg(2))
    if size:
        c.write(dest, c.read(src, size))  # lecture complète avant écriture : sûr pour memmove
    return dest


@hle("crt")
def memcmp(c):
    size = _bounded(c.arg(2))
    left, right = c.read(c.arg(0), size), c.read(c.arg(1), size)
    return 0 if left == right else (1 if left > right else -1 & 0xFFFF_FFFF)


def _length(c, address, unit):
    """Compte les unités jusqu'au zéro final, sans décoder.

    Lit page par page : une chaîne proche de la fin d'une zone mappée ne doit
    pas faire lire la page suivante, qui peut ne pas exister.
    """
    offset, terminator = 0, bytes(unit)
    while offset < MAX_COPY:
        cursor = address + offset
        size = STRING_CHUNK - cursor % STRING_CHUNK
        size -= size % unit or 0
        chunk = c.read(cursor, max(size, unit))
        for i in range(0, len(chunk) - unit + 1, unit):
            if chunk[i:i + unit] == terminator:
                return (offset + i) // unit
        offset += len(chunk) - len(chunk) % unit
    raise ValueError("Chaîne sans zéro final.")


@hle("crt")
def strlen(c):
    return _length(c, c.arg(0), 1)


@hle("crt")
def wcslen(c):
    return _length(c, c.arg(0), 2)


@hle("crt", "exit", "_exit", "quick_exit", "_Exit")
def exit_now(c):
    c.machine.request_exit(c.arg(0) & 0xFFFF_FFFF)


@hle("crt", "abort", "terminate")
def abort(c):
    c.machine.request_exit(ABORT_CODE)


@hle("crt")
def _initialize_onexit_table(c):
    """_onexit_table_t = {first, last, end} : une table vide est trois pointeurs nuls."""
    if not c.arg(0):
        return -1 & 0xFFFF_FFFF
    c.write(c.arg(0), bytes(24))
    return 0


@hle("crt", "_register_onexit_function", "_execute_onexit_table", "_crt_atexit",
     "atexit", "_cexit", "_c_exit")
def onexit(c):
    return 0  # la machine ne s'arrête jamais proprement : rien à exécuter à la sortie


@hle("crt")
def __acrt_iob_func(c):
    """Renvoie stdin/stdout/stderr : trois blocs FILE opaques alloués une fois."""
    streams = c.machine.crt_streams
    if streams is None:
        streams = c.machine.crt_streams = c.heap.alloc(FILE_SIZE * 3)
    return streams + FILE_SIZE * min(c.arg(0) & 0xFFFF_FFFF, 2)


@hle("crt")
def _get_stream_buffer_pointers(c):
    """Donne les adresses des champs _ptr, _base et _cnt du FILE (UCRT x64)."""
    stream = c.arg(0)
    for out, offset in ((c.arg(1), 8), (c.arg(2), 0), (c.arg(3), 16)):
        if out:
            c.write_u64(out, stream + offset)
    return 0


@hle("crt")
def setlocale(c):
    locale = c.machine.c_locale
    if locale is None:
        locale = c.machine.c_locale = c.heap.alloc(2)
        c.write(locale, b"C\0")
    return locale  # seule la locale « C » existe dans la machine émulée


@hle("crt")
def ___lc_codepage_func(c):
    return 0  # page de code de la locale « C »


@hle("crt", "___mb_cur_max_func", "__mb_cur_max_func")
def mb_cur_max(c):
    return 1  # la locale « C » code chaque caractère sur un octet


@hle("crt")
def ___lc_locale_name_func(c):
    """Tableau de six noms de locale (un par catégorie LC_*) : nuls pour la locale « C »."""
    names = c.machine.locale_names
    if names is None:
        names = c.machine.locale_names = c.heap.alloc(8 * LC_CATEGORIES)
    return names


def _ctype_flags(code):
    char = chr(code)
    flags = 0
    if char.isascii():
        flags |= CT_UPPER if char.isupper() else 0
        flags |= CT_LOWER if char.islower() else 0
        flags |= CT_DIGIT if char.isdigit() else 0
        flags |= CT_SPACE if char in " \t\n\r\v\f" else 0
        flags |= CT_PUNCT if 33 <= code <= 126 and not char.isalnum() else 0
        flags |= CT_CONTROL if code < 32 or code == 127 else 0
        flags |= CT_BLANK if char == " " else 0
        flags |= CT_HEX if char in "0123456789abcdefABCDEF" else 0
        flags |= CT_ALPHA if char.isalpha() else 0
    return flags


@hle("crt")
def __pctype_func(c):
    """Table de classification des caractères ; l'entrée -1 correspond à EOF."""
    table = c.machine.ctype_table
    if table is None:
        data = struct.pack("<257H", 0, *(_ctype_flags(code) for code in range(256)))
        table = c.machine.ctype_table = c.heap.alloc(len(data))
        c.write(table, data)
    return table + 2


@hle("crt", "_lock_locales", "_unlock_locales")
def locale_lock(c):
    return None


@hle("crt", "_configure_narrow_argv",
     "_initialize_narrow_environment", "_set_app_type", "_seh_filter_exe",
     "__setusermatherr", "_set_fmode", "_configthreadlocale")
def crt_startup(c):
    return 0  # étapes de démarrage du CRT : le prototype n'a ni argv ni environnement
