"""Compile les DLL console de TomBox (sous-module third_party/tombox) depuis leur dépôt public,
puis les copie dans runtime/build/tombox, dossier que le runtime d'AnyXboxX lit par défaut.

Leur crate compat-common incorpore à la compilation des données de serveurs privés de jeux
(`backend_data/`, profils et vidéos Fortnite, routes Trackmania et XDefiant) qui ne sont pas
publiées. Ce script crée des fichiers vides à leur place, uniquement sous `backend_data/`, pour
que la compilation aboutisse ; seule l'émulation en ligne de ces jeux en dépend. Aucun fichier
de jeu n'est créé ni récupéré.

    python tools/build_tombox.py            # les DLL par défaut (DEFAULT_CRATES)
    python tools/build_tombox.py d3d12_xs   # d'autres crates seulement
"""
import re
import shutil
import subprocess
import sys
import threading
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
TOMBOX = ROOT / "third_party" / "tombox"
OUTPUT = TOMBOX / "target" / "release"
INSTALL = ROOT / "runtime" / "build" / "tombox"
DEFAULT_CRATES = ("d3d12_xs", "xg_xs", "xmem", "d3d12_x", "xaudio2_9", "xapu", "xgs12", "xcrdapi", "acphal", "chad")
MISSING = re.compile(r"couldn't read `([^`]+)`")
MAX_ROUNDS = 5
TIMEOUT = 3600


def _cargo(crates):
    command = ["cargo", "build", "--release", *(arg for crate in crates for arg in ("-p", crate))]
    return _run(command, TOMBOX)


def _forward(stream, sink, lines):
    """Recopie chaque ligne de `stream` dans `sink` dès qu'elle arrive, et la garde dans `lines`."""
    for line in stream:
        sink.write(line)
        sink.flush()
        lines.append(line)


def _run(command, cwd, sink=None):
    """Lance `command` en affichant sa sortie d'erreur en direct (cargo y écrit sa progression) ;
    renvoie le code de retour et cette sortie, lue ensuite pour trouver les fichiers manquants."""
    sink = sink or sys.stderr
    lines = []
    with subprocess.Popen(command, cwd=cwd, stderr=subprocess.PIPE, text=True, encoding="utf-8",
                          errors="replace") as process:
        reader = threading.Thread(target=_forward, args=(process.stderr, sink, lines), daemon=True)
        reader.start()
        try:
            returncode = process.wait(timeout=TIMEOUT)
        except subprocess.TimeoutExpired:
            process.kill()
            raise
        finally:
            reader.join()
    return subprocess.CompletedProcess(command, returncode, stderr="".join(lines))


def _placeholder(raw):
    path = Path(raw)
    path = path if path.is_absolute() else (TOMBOX / path)
    resolved = path.resolve()
    if "backend_data" not in resolved.parts or TOMBOX.resolve() not in resolved.parents:
        raise RuntimeError(f"fichier manquant hors de backend_data, non créé : {raw}")
    resolved.parent.mkdir(parents=True, exist_ok=True)
    resolved.touch()
    return resolved


def build(crates):
    """Compile `crates` ; renvoie la liste des fichiers vides créés."""
    created = []
    for _ in range(MAX_ROUNDS):
        result = _cargo(crates)
        if result.returncode == 0:
            return created
        missing = MISSING.findall(result.stderr)
        if not missing:
            raise RuntimeError(f"cargo build a échoué :\n{result.stderr[-4000:]}")
        created += [_placeholder(path) for path in missing]
    raise RuntimeError("compilation toujours en échec après création des fichiers vides")


def _compiled_dll(crate, output):
    """DLL produite par `crate` (le nom de bibliothèque peut différer par la casse : XAudio2_9)."""
    matches = [path for path in output.glob("*.dll") if path.stem.lower() == crate.lower()]
    if not matches:
        raise RuntimeError(f"{crate}.dll absente de {output} après compilation")
    return matches[0]


def install(crates, output=OUTPUT, destination=INSTALL):
    """Copie les DLL compilées dans `destination`, en minuscules ; renvoie les fichiers copiés."""
    destination.mkdir(parents=True, exist_ok=True)
    copied = []
    for crate in crates:
        target = destination / f"{crate.lower()}.dll"
        shutil.copy2(_compiled_dll(crate, output), target)
        copied.append(target)
    return copied


def main(argv):
    if argv in (["-h"], ["--help"]):
        print(__doc__)
        return 0
    crates = tuple(argv) or DEFAULT_CRATES
    created = build(crates)
    copied = install(crates)
    print(f"{len(created)} fichier(s) vide(s) créé(s) sous backend_data ; {len(copied)} DLL dans {INSTALL} : "
          f"{', '.join(crates)}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
