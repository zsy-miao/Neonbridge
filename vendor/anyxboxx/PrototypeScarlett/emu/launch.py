"""Lancement isolé (Unicorn) d'un paquet GDK ou d'un exécutable : `--lancer`.

Le programme ne touche jamais le PC : chaque appel système passe par la couche
HLE. Le compte rendu liste les imports non résolus et les appels non émulés,
dans l'ordre utile pour compléter l'émulateur.
"""
from pathlib import Path
import xml.etree.ElementTree as ET

import pefile
from unicorn import UcError

from .config import read_package
from .machine import Machine
from .report import analyse

ERRORS = (ValueError, OSError, ET.ParseError, pefile.PEFormatError, UcError)
MAX_LISTED = 30


def resolve(target):
    """Dossier de paquet → (exécutable Scarlett, Title ID) ; exécutable → (lui-même, 0)."""
    target = Path(target)
    if target.is_dir():
        package = read_package(target)
        return package.scarlett_entry(), package.title_id
    return target, 0


def _missing_calls(machine, log):
    calls = sorted(machine.missing_calls.items(), key=lambda item: -item[1])
    if not calls:
        log("Aucun appel non émulé.")
        return
    log(f"Appels non émulés ({len(calls)}) :")
    for name, count in calls[:MAX_LISTED]:
        log(f"  {name} ×{count}")
    if len(calls) > MAX_LISTED:
        log(f"  … et {len(calls) - MAX_LISTED} autres")


def markdown_report(exe, report, missing_calls, outcome):
    """Rapport Markdown : imports non résolus groupés par DLL, puis appels non émulés par fréquence."""
    by_dll = {}
    for name in report.missing:
        dll, _, function = name.partition("!")
        by_dll.setdefault(dll, []).append(function)
    lines = [f"# Rapport AnyXboxX : {exe.name}", "",
             f"Imports : {report.total} · résolus : {report.coverage:.1f} % · {outcome}", "",
             "## Imports non résolus, par DLL", "", "| DLL | Nombre | Fonctions |", "|---|---|---|"]
    for dll, functions in sorted(by_dll.items(), key=lambda item: (-len(item[1]), item[0].lower())):
        lines.append(f"| {dll} | {len(functions)} | {', '.join(sorted(functions))} |")
    lines += ["", "## Appels non émulés pendant l'exécution", "", "| Fonction | Appels |", "|---|---|"]
    for name, count in sorted(missing_calls.items(), key=lambda item: (-item[1], item[0])):
        lines.append(f"| {name} | {count} |")
    return "\n".join(lines) + "\n"


def run(target, extra_dirs=(), log=print, markdown=None):
    """Lance `target` dans la machine isolée. Renvoie 0 si l'exécution s'est déroulée.

    `markdown` : chemin d'un rapport à écrire (liste de travail pour compléter l'émulateur).
    """
    try:
        exe, title_id = resolve(target)
        dirs = (exe.parent, *(Path(d) for d in extra_dirs))
        report = analyse(exe, dirs[1:])
        log(report.text())
        log(f"[lancer] {exe.name} · Title ID {title_id:#010x} · machine isolée")
        machine = Machine(exe, title_id=title_id, log=log, search_dirs=dirs)
    except ERRORS as error:
        log(f"Erreur : {error}")
        return 1
    try:
        code = machine.start() & 0xFFFF_FFFF
        outcome = f"terminé, retour {code:#x}"
        status = 0
    except ERRORS as error:
        outcome = f"arrêt : {error}"
        status = 1
    log(f"[lancer] {exe.name} : {outcome}")
    _missing_calls(machine, log)
    if markdown is not None:
        Path(markdown).write_text(markdown_report(exe, report, machine.missing_calls, outcome), encoding="utf-8")
        log(f"[lancer] rapport écrit : {markdown}")
    return status
