"""Couche HLE : l'import des modules enregistre leurs gestionnaires."""
from . import crt, crt_startup, crt_text, gdk, kernel, kernel_misc, libm, library, xmem  # noqa: F401  (enregistrement par décorateur)
from .core import E_NOTIMPL, REGISTRY, Call, family, lookup
from .guest import GUEST_CODE


def resolve(dll, name):
    """Renvoie ("code", octets), ("python", gestionnaire) ou ("absent", None)."""
    key = (family(dll), name)
    if key in GUEST_CODE:
        return "code", GUEST_CODE[key]
    handler = lookup(dll, name)
    return ("python", handler) if handler else ("absent", None)


__all__ = ["Call", "E_NOTIMPL", "REGISTRY", "GUEST_CODE", "family", "resolve"]
