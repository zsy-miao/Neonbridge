"""Banc d'essai : charge chaque module PE d'un dossier et exécute son initialisation."""
from dataclasses import dataclass

from unicorn import UcError
import pefile

from .machine import Machine
from .report import analyse

PATTERNS = ("*.dll", "*.exe")
PE_SIGNATURE = b"PE\0\0"
PE32_PLUS_MAGIC = (0x20B).to_bytes(2, "little")
ERRORS = (ValueError, OSError, UcError, pefile.PEFormatError)


@dataclass(frozen=True)
class BenchRow:
    name: str
    imports: int
    coverage: float
    loaded: bool
    initialised: bool
    detail: str


def run_one(path, extra_dirs=()):
    """Charge `path` avec ses DLL (même dossier puis `extra_dirs`), initialise le tout.

    Une DLL réussit si son DllMain renvoie TRUE ; un exécutable, s'il se termine
    sans erreur.
    """
    try:
        report = analyse(path, extra_dirs)
    except ERRORS as error:
        return BenchRow(path.name, 0, 0.0, False, False, f"analyse : {error}")
    try:
        machine = Machine(path, search_dirs=(path.parent, *extra_dirs))
    except ERRORS as error:
        return BenchRow(path.name, report.total, report.coverage, False, False, str(error))
    try:
        code = machine.start() & 0xFFFF_FFFF
        detail = f"retour {code:#x}"
        ok = code == 1 if machine.image.is_dll else True
    except ERRORS as error:
        detail, ok = str(error), False
    linked = len(machine.modules.modules) - 1
    if linked:
        detail += f" · {linked} DLL liée(s)"
    missing = len(machine.missing_calls)
    if missing:
        detail += f" · {missing} appel(s) non émulé(s)"
    return BenchRow(path.name, report.total, report.coverage, True, ok, detail)


def is_pe32_plus(path):
    """Vrai pour un module PE32+ (x86-64) ; un lanceur 32 bits ou .NET n'est pas un module du jeu."""
    try:
        with path.open("rb") as file:
            header = file.read(0x400)
    except OSError:
        return False
    if header[:2] != b"MZ" or len(header) < 0x40:
        return False
    pe = int.from_bytes(header[0x3C:0x40], "little")
    return (pe + 26 <= len(header) and header[pe:pe + 4] == PE_SIGNATURE
            and header[pe + 24:pe + 26] == PE32_PLUS_MAGIC)


def run(folder, extra_dirs=()):
    paths = sorted({p for pattern in PATTERNS for p in folder.glob(pattern)},
                   key=lambda p: p.name.lower())
    return tuple(run_one(path, extra_dirs) for path in paths
                 if path.is_file() and is_pe32_plus(path))


def table(rows):
    lines = ["| Module | Imports | Résolus | Chargé | Init. | Détail |",
             "|---|---|---|---|---|---|"]
    for r in rows:
        lines.append(f"| {r.name} | {r.imports} | {r.coverage:.0f} % | "
                     f"{'oui' if r.loaded else 'non'} | {'oui' if r.initialised else 'non'} "
                     f"| {r.detail} |")
    total = sum(r.imports for r in rows)
    emulated = sum(round(r.imports * r.coverage / 100) for r in rows)
    ok = sum(r.initialised for r in rows)
    share = 100 * emulated / total if total else 0
    lines.append(f"\nInitialisation réussie : {ok}/{len(rows)} · imports résolus : "
                 f"{emulated}/{total} ({share:.0f} %)")
    return "\n".join(lines)
