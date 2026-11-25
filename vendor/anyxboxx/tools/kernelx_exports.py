"""Redirections de kernelx.dll (noyau de la Xbox) vers kernel32 et kernelbase du PC.

Les binaires console importent kernelx.dll là où un programme PC importe
kernel32.dll : pour toutes les fonctions communes, le kernelx.dll d'AnyXboxX
redirige vers celle de Windows. Les fonctions propres à la console sont écrites
à part (runtime/kernelx/kernelx.cpp), au fur et à mesure que les rapports
(`scarlett.py --lancer … --md`) les révèlent.

    python tools/kernelx_exports.py [--out runtime/kernelx/kernelx_forwarders.inc]
"""
import argparse
import os
import sys
from pathlib import Path

import pefile

REPOSITORY = Path(__file__).resolve().parents[1]
DEFAULT_OUT = REPOSITORY / "runtime" / "kernelx" / "kernelx_forwarders.inc"
SOURCES = ("kernel32", "kernelbase")   # kernel32 d'abord : c'est l'interface publique
DIRECTORY_EXPORT = 0


def exported_names(path):
    """Noms exportés par une DLL (sans les exports par ordinal seul)."""
    with pefile.PE(str(path), fast_load=True) as pe:
        pe.parse_data_directories(directories=[DIRECTORY_EXPORT])
        directory = getattr(pe, "DIRECTORY_ENTRY_EXPORT", None)
        return [symbol.name.decode() for symbol in directory.symbols if symbol.name] if directory else []


def forwarders(sources):
    """{nom: dll} : chaque nom est servi par la première DLL de `sources` qui l'exporte."""
    table = {}
    for dll, names in sources:
        for name in names:
            if name.isidentifier():
                table.setdefault(name, dll)
    return dict(sorted(table.items()))


def to_include(table, origin):
    lines = ["// Généré par tools/kernelx_exports.py — ne pas modifier à la main.",
             f"// Source : {origin}",
             f"// {len(table)} fonctions de kernelx.dll redirigées vers Windows."]
    lines += [f'#pragma comment(linker, "/export:{name}={dll}.{name}")' for name, dll in table.items()]
    return "\n".join(lines) + "\n"


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--out", type=Path, default=DEFAULT_OUT)
    args = parser.parse_args(argv)
    system = Path(os.environ.get("SystemRoot", r"C:\Windows")) / "System32"
    sources = [(dll, exported_names(system / f"{dll}.dll")) for dll in SOURCES]
    table = forwarders(sources)
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(to_include(table, "System32 : " + ", ".join(f"{d}.dll" for d in SOURCES)),
                        encoding="utf-8", newline="\n")
    print(f"{len(table)} redirections → {args.out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
