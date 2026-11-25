"""CRT : locale, conversions de nombres et chaînes larges."""
import math
import re
import struct

from .core import MASK64, hle

CHAR_MAX = 127
LCONV_POINTERS = 10          # decimal_point … negative_sign
LCONV_CHARS = 8              # int_frac_digits … n_sign_posn
LCONV_WIDE_POINTERS = 8      # _W_decimal_point … _W_negative_sign
EINVAL, ERANGE = 22, 34
NUMBER_LIMIT = 512
INTEGER = re.compile(r"\s*([+-]?)(0[xX][0-9a-fA-F]+|0[0-7]*|[1-9][0-9]*)")
FLOAT = re.compile(r"\s*[+-]?(inf(inity)?|nan|(\d+\.?\d*|\.\d+)([eE][+-]?\d+)?)", re.IGNORECASE)


def _static_text(c, key, value, wide=False):
    from .crt_startup import _cell

    data = value.encode("utf-16-le") + b"\0\0" if wide else value.encode() + b"\0"
    return _cell(c, key, len(data), data)


@hle("crt")
def _wsetlocale(c):
    return _static_text(c, "locale-w", "C", wide=True)


@hle("crt")
def localeconv(c):
    """struct lconv de la locale « C » : point décimal, tout le reste vide."""
    from .crt_startup import _cell

    dot, empty = _static_text(c, "lconv-dot", "."), _static_text(c, "lconv-empty", "")
    wide_dot, wide_empty = _static_text(c, "lconv-wdot", ".", True), _static_text(c, "lconv-wempty", "", True)
    data = struct.pack(f"<{LCONV_POINTERS}Q", dot, *[empty] * (LCONV_POINTERS - 1))
    data += bytes([CHAR_MAX] * LCONV_CHARS)
    data += struct.pack(f"<{LCONV_WIDE_POINTERS}Q", wide_dot, *[wide_empty] * (LCONV_WIDE_POINTERS - 1))
    return _cell(c, "lconv", len(data), data)


def _parse(c, pattern):
    """Lit au plus NUMBER_LIMIT octets et renvoie (correspondance, texte)."""
    text = c.cstr(c.arg(0), NUMBER_LIMIT)
    return pattern.match(text), text


def _set_end(c, pointer_index, consumed):
    end = c.arg(pointer_index)
    if end:
        c.write_u64(end, c.arg(0) + consumed)


def _strto_integer(c, signed):
    base = c.arg(2) & 0xFFFF_FFFF
    match, _ = _parse(c, INTEGER)
    if match is None or base not in (0, 8, 10, 16):
        _set_end(c, 1, 0)
        return 0
    sign, digits = match.group(1), match.group(2)
    try:
        value = int(digits, 0) if base == 0 else int(digits, base)
    except ValueError:
        value = int(digits.lstrip("0") or "0", 10)
    value = -value if sign == "-" else value
    _set_end(c, 1, match.end())
    limit = (1 << 63) if signed else (1 << 64)
    return max(min(value, limit - 1), -limit if signed else -(limit - 1)) & MASK64


@hle("crt", "strtoll", "_strtoi64")
def strtoll(c):
    return _strto_integer(c, signed=True)


@hle("crt", "strtoull", "_strtoui64")
def strtoull(c):
    return _strto_integer(c, signed=False)


@hle("crt", "strtod", "atof")
def strtod(c):
    match, _ = _parse(c, FLOAT)
    _set_end(c, 1, match.end() if match else 0)
    c.return_double(float(match.group(0).strip()) if match else 0.0)
    return 0


@hle("crt")
def _dsign(c):
    return int(math.copysign(1.0, c.arg_double(0)) < 0)


@hle("crt")
def wcscat_s(c):
    dest, size, src = c.arg(0), c.arg(1), c.arg(2)
    if not dest or not src or size == 0:
        return EINVAL
    current, extra = c.wstr(dest, size), c.wstr(src, size)
    if len(current) + len(extra) + 1 > size:
        c.write(dest, b"\0\0")
        return ERANGE
    c.write(dest + 2 * len(current), (extra + "\0").encode("utf-16-le"))
    return 0


@hle("crt", "_lock_file", "_unlock_file")
def file_lock(c):
    return None   # un seul thread


@hle("crt", "setvbuf", "fflush")
def stream_success(c):
    return 0
