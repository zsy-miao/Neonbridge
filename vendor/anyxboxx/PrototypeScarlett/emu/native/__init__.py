"""Exécution native façon Wine (Windows uniquement). Voir image.py pour l'avertissement."""
import os
import sys

if sys.platform != "win32":
    raise ImportError("L'exécution native d'AnyXboxX demande Windows x64.")

from .image import NativeImage  # noqa: E402
from .shims import adopt_command_line  # noqa: E402


def run(path, search_dirs=None, log=print):
    """Charge et lance `path` ; un exécutable avec CRT termine le processus à sa sortie."""
    image = NativeImage(path, search_dirs, log)
    log(f"[natif] {image.path.name} chargé à {image.base:#x} : exécution directe sur le processeur.")
    os.chdir(image.path.parent)   # le jeu lit ses fichiers en chemins relatifs
    os.environ["ANYXBOXX_GAME_DIR"] = str(image.path.parent)   # MicrosoftGame.config pour le runtime C++
    if not adopt_command_line(image.path):
        log("[natif] ligne de commande du jeu non appliquée : il verra celle de python.")
    return image.call_entry()


__all__ = ["NativeImage", "run"]
