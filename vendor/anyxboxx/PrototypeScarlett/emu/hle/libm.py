"""Fonctions mathématiques de l'UCRT (api-ms-win-crt-math-*) : doubles en XMM0-XMM1."""
import math

from .core import hle

NAN = float("nan")


def _safe(function, *values):
    try:
        return function(*values)
    except (ValueError, OverflowError):
        return NAN  # le CRT renvoie NaN ou ±inf ; NaN suffit au prototype


UNARY = {
    "exp": math.exp, "log": math.log, "log10": math.log10, "log2": math.log2,
    "sqrt": math.sqrt, "floor": math.floor, "ceil": math.ceil, "fabs": math.fabs,
    "sin": math.sin, "cos": math.cos, "tan": math.tan,
    "asin": math.asin, "acos": math.acos, "atan": math.atan,
}
BINARY = {"pow": math.pow, "fmod": math.fmod, "atan2": math.atan2}


def _unary(function):
    def handler(c):
        c.return_double(float(_safe(function, c.arg_double(0))))
    return handler


def _binary(function):
    def handler(c):
        c.return_double(float(_safe(function, c.arg_double(0), c.arg_double(1))))
    return handler


for _name, _function in UNARY.items():
    hle("crt", _name)(_unary(_function))
for _name, _function in BINARY.items():
    hle("crt", _name)(_binary(_function))
