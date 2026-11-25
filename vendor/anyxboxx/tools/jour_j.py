"""Premier lancement d'un exécutable console : tous les diagnostics en une commande.

    python tools/jour_j.py <dossier du paquet | jeu.exe> [--natif] [--sortie <dossier>] [--duree <s>]

1. Inventaire statique de l'exécutable (empreinte, en-têtes, sections, TLS, imports).
2. Pour chaque fonction importée, qui la fournit : le paquet, AnyXboxX, TomBox, Windows ou personne.
3. Analyse isolée (`scarlett.py --rapport`) et lancement isolé dans Unicorn (`--lancer --md`).
4. Avec --natif seulement : lancement réel par anyxboxx.exe avec les DLL de TomBox. Le code du jeu
   s'exécute alors sur ce PC sans isolation ; réservé aux exécutables dont on connaît la provenance.

Le rapport est écrit hors du dépôt (par défaut %LOCALAPPDATA%\\AnyXboxX\\jour-j\\<date>) : il nomme des
fonctions du jeu et ne doit pas être versionné.
"""
import hashlib
import os
import subprocess
import sys
import time
from collections import Counter, defaultdict
from pathlib import Path
from tempfile import mkdtemp

import pefile
import pso_inventory

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "PrototypeScarlett"))
from emu.launch import resolve  # noqa: E402
from emu.modules import dll_name, is_system  # noqa: E402

SCARLETT = ROOT / "PrototypeScarlett" / "scarlett.py"
RUNTIME = Path(os.environ.get("ANYXBOXX_RUNTIME") or ROOT / "runtime" / "build" / "bin")
TOMBOX = Path(os.environ.get("ANYXBOXX_TOMBOX_DIR") or ROOT / "third_party" / "tombox" / "target" / "release")
SYSTEM32 = Path(os.environ.get("SystemRoot", r"C:\Windows")) / "System32"
PROVIDERS = ("paquet", "AnyXboxX", "TomBox", "Windows")
MISSING = "manquant"
MAX_LISTED = 25
ISOLATED_TIMEOUT = 600
NATIVE_WARNING = ("ATTENTION : --natif exécute le code du jeu sur ce PC sans isolation. "
                  "À réserver à un exécutable dont la provenance est connue.")
_EXPORTS = {}


def _exports(path):
    """Noms exportés d'une DLL (ensemble vide si illisible), mis en cache par chemin."""
    if path not in _EXPORTS:
        try:
            pe = pefile.PE(str(path), fast_load=True)
            pe.parse_data_directories([pefile.DIRECTORY_ENTRY["IMAGE_DIRECTORY_ENTRY_EXPORT"]])
            symbols = getattr(pe, "DIRECTORY_ENTRY_EXPORT", None)
            _EXPORTS[path] = {s.name.decode() for s in symbols.symbols if s.name} if symbols else set()
        except (OSError, pefile.PEFormatError):
            _EXPORTS[path] = set()
    return _EXPORTS[path]


def _dll_folders(package):
    return {"paquet": package, "AnyXboxX": RUNTIME, "TomBox": TOMBOX, "Windows": SYSTEM32}


def provider(dll, function, folders):
    """Premier fournisseur de dll!function, dans l'ordre de résolution du lanceur."""
    name = dll_name(dll)
    for label in PROVIDERS:
        if label == "Windows" and name.startswith(("api-ms-", "ext-ms-")):
            return label                    # ensembles d'API : résolus par Windows
        path = folders[label] / name
        if path.is_file() and (function.startswith("#") or function in _exports(path)):
            return label
    return "Windows" if is_system(name) and not (SYSTEM32 / name).is_file() else MISSING


def _section_row(section):
    name = section.Name.rstrip(b"\0").decode(errors="replace")
    return f"| `{name}` | {section.Misc_VirtualSize:,} | {section.SizeOfRawData:,} |".replace(",", " ")


def inventory(exe):
    data = exe.read_bytes()
    pe = pefile.PE(data=data)
    imports = [(entry.dll.decode(), imp.name.decode() if imp.name else f"#{imp.ordinal}")
               for entry in getattr(pe, "DIRECTORY_ENTRY_IMPORT", []) for imp in entry.imports]
    header = pe.OPTIONAL_HEADER
    lines = [f"- Fichier : `{exe.name}`, {len(data):,} octets".replace(",", " "),
             f"- SHA-256 : `{hashlib.sha256(data).hexdigest()}`",
             f"- Machine 0x{pe.FILE_HEADER.Machine:04X}, sous-système {header.Subsystem}, "
             f"base 0x{header.ImageBase:X}, point d'entrée 0x{header.AddressOfEntryPoint:X}",
             f"- TLS : {'oui' if hasattr(pe, 'DIRECTORY_ENTRY_TLS') else 'non'}",
             f"- Imports : {len(imports)} fonctions dans {len({d.lower() for d, _ in imports})} DLL",
             "", "| Section | Taille virtuelle | Taille brute |", "|---|---:|---:|"]
    return imports, lines + [_section_row(section) for section in pe.sections]


def coverage(imports, package):
    folders = _dll_folders(package)
    by_dll = defaultdict(Counter)
    missing = defaultdict(list)
    for dll, function in imports:
        found = provider(dll, function, folders)
        by_dll[dll_name(dll)][found] += 1
        if found == MISSING:
            missing[dll_name(dll)].append(function)
    labels = (*PROVIDERS, MISSING)
    totals = Counter()
    lines = ["| DLL | Imports | " + " | ".join(label.capitalize() for label in labels) + " |",
             "|---|---:|" + "---:|" * len(labels)]
    for dll, counts in sorted(by_dll.items(), key=lambda item: -item[1][MISSING]):
        totals.update(counts)
        lines.append(f"| `{dll}` | {sum(counts.values())} | " + " | ".join(str(counts[x]) for x in labels) + " |")
    lines.append(f"| **Total** | {sum(totals.values())} | " + " | ".join(str(totals[x]) for x in labels) + " |")
    for dll, functions in sorted(missing.items()):
        shown = ", ".join(f"`{f}`" for f in functions[:MAX_LISTED])
        more = f" … et {len(functions) - MAX_LISTED} autres" if len(functions) > MAX_LISTED else ""
        lines.append(f"\n- `{dll}` manquantes : {shown}{more}")
    return totals, lines


def _text(raw):
    return raw.decode("utf-8", errors="replace") if isinstance(raw, bytes) else (raw or "")


def _run(command, timeout, environment=None):
    started = time.perf_counter()
    try:
        result = subprocess.run(command, capture_output=True, timeout=timeout, env=environment, cwd=ROOT)
        outcome = f"code de sortie {result.returncode}"
        output = (_text(result.stdout) + _text(result.stderr)).strip()
    except subprocess.TimeoutExpired as expired:
        outcome = f"toujours en cours après {timeout} s (arrêté)"
        output = (_text(expired.stdout) + _text(expired.stderr)).strip()
    tail = "\n".join(output.splitlines()[-60:])
    return outcome, f"{time.perf_counter() - started:.1f} s", tail


def isolated_steps(target, output):
    lancer_md = output / "lancement-isole.md"
    lines = []
    for title, command in (("Analyse isolée (--rapport)", [sys.executable, str(SCARLETT), "--rapport", str(target)]),
                           ("Lancement isolé dans Unicorn (--lancer)",
                            [sys.executable, str(SCARLETT), "--lancer", str(target), "--md", str(lancer_md)])):
        outcome, duration, tail = _run(command, ISOLATED_TIMEOUT, {**os.environ, "PYTHONIOENCODING": "utf-8"})
        lines += [f"## {title}", "", f"{outcome}, {duration}.", "", "```", tail, "```", ""]
    if lancer_md.is_file():
        lines.append(f"Rapport détaillé : `{lancer_md}`")
    return lines


def native_step(target, duration, output):
    launcher = RUNTIME / "anyxboxx.exe"
    if not launcher.is_file():
        return ["## Lancement natif", "", f"`{launcher}` absent : compilez runtime/ avec CMake."]
    output = output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    log = Path(mkdtemp(prefix="tombox-", dir=output)) / "compat.log"
    environment = {**os.environ, "ANYXBOXX_TRACE": "1", "TOMBOX_LOG": str(log)}
    if TOMBOX.is_dir():
        environment["ANYXBOXX_TOMBOX_DIR"] = str(TOMBOX)
    outcome, elapsed, tail = _run([str(launcher), str(target)], duration, environment)
    return ["## Lancement natif (anyxboxx.exe)", "", f"> {NATIVE_WARNING}", "",
            f"DLL TomBox : `{environment.get('ANYXBOXX_TOMBOX_DIR', 'non')}`. Résultat : {outcome}, {elapsed}.",
            "", "```", tail, "```", "", *pso_inventory.summary(log, output)]


def report(target, output, native=False, duration=60):
    exe, _ = resolve(Path(target))
    if not exe.is_file():
        raise FileNotFoundError(f"exécutable du jeu absent : {exe}")
    imports, inventory_lines = inventory(exe)
    totals, coverage_lines = coverage(imports, exe.parent)
    covered = sum(totals.values()) - totals[MISSING]
    lines = [f"# Premier lancement : {exe.name}", "",
             f"Imports couverts : **{covered}/{sum(totals.values())}** ; manquants : **{totals[MISSING]}**.", "",
             "## Inventaire", "", *inventory_lines, "", "## Qui fournit chaque import", "", *coverage_lines, "",
             *isolated_steps(target, output)]
    if native:
        lines += native_step(target, duration, output)
    return "\n".join(lines) + "\n"


def _option(argv, name, default):
    return argv[argv.index(name) + 1] if name in argv and argv.index(name) + 1 < len(argv) else default


def main(argv):
    if not argv or argv[0].startswith("--"):
        print(__doc__)
        return 2
    stamp = time.strftime("%Y-%m-%d_%H%M%S")
    default = Path(os.environ.get("LOCALAPPDATA", Path.home())) / "AnyXboxX" / "jour-j" / stamp
    output = Path(_option(argv, "--sortie", default))
    output.mkdir(parents=True, exist_ok=True)
    if "--natif" in argv:
        print(NATIVE_WARNING, file=sys.stderr)
    try:
        text = report(argv[0], output, native="--natif" in argv, duration=int(_option(argv, "--duree", 60)))
    except (OSError, ValueError, pefile.PEFormatError) as error:
        print(f"Erreur : {error}", file=sys.stderr)
        return 1
    path = output / "rapport.md"
    path.write_text(text, encoding="utf-8")
    print(text.splitlines()[2])
    print(f"Rapport : {path}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
