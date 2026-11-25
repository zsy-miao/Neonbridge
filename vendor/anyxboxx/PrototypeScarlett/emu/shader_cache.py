"""Cache disque des shaders compilés (HLSL → DXIL), pour ne payer dxc qu'une fois par shader.

Clé : SHA-256 de la version du cache, du modèle de shader, de l'identité de dxc (chemin, taille,
date) et du texte HLSL ; une mise à jour de dxc ou du recompilateur invalide donc les entrées.
Une couche mémoire évite le disque pour les shaders déjà vus dans le processus.
Dossier : ANYXBOXX_SHADER_CACHE, sinon %LOCALAPPDATA%\\AnyXboxX\\shader-cache. Écriture atomique ;
une entrée qui n'est pas un conteneur DXIL est écartée ; au-delà de ANYXBOXX_SHADER_CACHE_MB
(256 par défaut), les entrées les plus anciennes sont supprimées.
"""
import hashlib
import os
from dataclasses import dataclass
from pathlib import Path

CACHE_VERSION = "anyxboxx-dxil-1"
DXIL_MAGIC = b"DXBC"
DEFAULT_LIMIT_MB = 256
MEGABYTE = 1024 * 1024
SUFFIX = ".dxil"


@dataclass
class Stats:
    hits: int = 0
    misses: int = 0


def default_folder():
    configured = os.environ.get("ANYXBOXX_SHADER_CACHE")
    if configured:
        return Path(configured)
    base = os.environ.get("LOCALAPPDATA") or Path.home() / "AppData" / "Local"
    return Path(base) / "AnyXboxX" / "shader-cache"


def compiler_identity(compiler):
    """Chemin, taille et date du compilateur : change quand dxc est mis à jour."""
    if compiler is None:
        return "aucun"
    info = Path(compiler).stat()
    return f"{Path(compiler).resolve()}|{info.st_size}|{int(info.st_mtime)}"


def cache_key(hlsl, shader_model, compiler):
    material = "\n".join((CACHE_VERSION, shader_model, compiler_identity(compiler), hlsl))
    return hashlib.sha256(material.encode("utf-8")).hexdigest()


class ShaderCache:
    def __init__(self, folder=None, limit_bytes=None):
        self.folder = Path(folder) if folder else default_folder()
        limit_mb = int(os.environ.get("ANYXBOXX_SHADER_CACHE_MB", DEFAULT_LIMIT_MB))
        self.limit = limit_bytes if limit_bytes is not None else limit_mb * MEGABYTE
        self.stats = Stats()
        self.memory = {}                    # shaders déjà vus dans ce processus
        self._total = None                  # taille du dossier, mesurée une fois puis tenue à jour

    def _path(self, key):
        return self.folder / key[:2] / f"{key}{SUFFIX}"

    def get(self, key):
        """DXIL en cache, ou None (entrée absente ou corrompue, alors supprimée)."""
        if key in self.memory:
            self.stats.hits += 1
            return self.memory[key]
        path = self._path(key)
        try:
            data = path.read_bytes()
        except OSError:
            self.stats.misses += 1
            return None
        if not data.startswith(DXIL_MAGIC):
            path.unlink(missing_ok=True)
            self.stats.misses += 1
            return None
        os.utime(path)                      # récemment utilisé : gardé en priorité
        self.stats.hits += 1
        self.memory[key] = data
        return data

    def put(self, key, dxil):
        if not dxil.startswith(DXIL_MAGIC):
            raise ValueError("ce n'est pas un conteneur DXIL")
        path = self._path(key)
        path.parent.mkdir(parents=True, exist_ok=True)
        temporary = path.with_suffix(f".{os.getpid()}.tmp")
        temporary.write_bytes(dxil)
        replaced = path.stat().st_size if path.exists() else 0
        os.replace(temporary, path)
        self.memory[key] = dxil
        if self._total is None:
            self._total = sum(entry.stat().st_size for entry in self.entries())
        else:
            self._total += len(dxil) - replaced
        if self._total > self.limit:
            self.prune()

    def entries(self):
        return sorted(self.folder.glob(f"*/*{SUFFIX}"), key=lambda p: p.stat().st_mtime)

    def prune(self):
        """Supprime les entrées les plus anciennes tant que le cache dépasse sa limite."""
        entries = self.entries()
        total = sum(p.stat().st_size for p in entries)
        for path in entries:
            if total <= self.limit:
                break
            total -= path.stat().st_size
            path.unlink(missing_ok=True)
        self._total = total
