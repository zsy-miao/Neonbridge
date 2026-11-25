"""Table des API du runtime GDK : fonction X* → (fournisseur, interface, n° de méthode).

Sur PC, chaque fonction X* de xgameruntime.lib (bibliothèque statique du GDK
public) appelle QueryApiImpl(&fournisseur, &interface, &objet), puis une méthode
de l'objet, de type COM (0 QueryInterface, 1 AddRef, 2 Release, puis les API).
Cet outil relit le désassemblage de la bibliothèque (dumpbin /disasm) et produit
la table, pour l'interopérabilité : runtime/xgameruntime/gdk_api_table.inc.

    python tools/gdk_interfaces.py [--lib xgameruntime.lib] [--out fichier.inc]
"""
import argparse
import collections
import glob
import os
import re
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path

REPOSITORY = Path(__file__).resolve().parents[1]
DEFAULT_OUT = REPOSITORY / "runtime" / "xgameruntime" / "gdk_api_table.inc"
GDK_LIBS = r"C:\Program Files (x86)\Microsoft GDK\*\windows\lib\x64\xgameruntime.lib"
DUMPBINS = r"C:\Program Files\Microsoft Visual Studio\*\*\VC\Tools\MSVC\*\bin\Hostx64\x64\dumpbin.exe"
RELEASE_OFFSET = 0x10          # IUnknown::Release, appelé après chaque API
POINTER_SIZE = 8

FUNCTION = re.compile(r"^(X[A-Za-z0-9_]*):\s*$", re.MULTILINE)
LEA_GUID = re.compile(r"lea\s+(rcx|rdx),\[_GUID_([0-9a-f_]{36})\]")
COPY_RCX = re.compile(r"mov\s+rdx,rcx\b")
VTABLE_CALL = re.compile(r"mov\s+rax,qword ptr \[rax\+([0-9A-F]+)h\]")
QUERY = "call        QueryApiImpl"


@dataclass(frozen=True)
class GdkApi:
    name: str
    provider: str
    interface: str
    method: int


def _guid(text):
    return text.replace("_", "-").upper()


def _parse_function(name, body):
    """Premier appel QueryApiImpl de la fonction : GUID de rcx/rdx avant, méthode après."""
    before, found, after = body.partition(QUERY)
    if not found:
        return None
    registers = {}
    for line in before.splitlines():
        lea = LEA_GUID.search(line)
        if lea:
            registers[lea.group(1)] = _guid(lea.group(2))
        elif COPY_RCX.search(line) and "rcx" in registers:
            registers["rdx"] = registers["rcx"]
    offsets = (int(value, 16) for value in VTABLE_CALL.findall(after.split(QUERY)[0]))
    offset = next((o for o in offsets if o != RELEASE_OFFSET), None)
    if "rcx" not in registers or "rdx" not in registers or offset is None:
        return None
    return GdkApi(name, registers["rcx"], registers["rdx"], offset // POINTER_SIZE)


def parse(disassembly, unreadable=None):
    """Toutes les API trouvées dans le texte de dumpbin /disasm, triées par nom.

    `unreadable` (liste) reçoit les fonctions qui appellent QueryApiImpl mais dont
    l'appel n'a pas pu être lu : sans cela, la table serait incomplète sans le dire.
    """
    starts = list(FUNCTION.finditer(disassembly))
    apis = {}
    for index, match in enumerate(starts):
        end = starts[index + 1].start() if index + 1 < len(starts) else len(disassembly)
        body = disassembly[match.end():end]
        api = _parse_function(match.group(1), body)
        if api is not None:
            apis.setdefault(api.name, api)
        elif QUERY in body and unreadable is not None:
            unreadable.append(match.group(1))
    return tuple(sorted(apis.values(), key=lambda api: api.name))


def groups(apis):
    """Nom lisible de chaque interface : préfixe commun de ses fonctions les plus nombreuses."""
    by_key = collections.defaultdict(list)
    for api in apis:
        by_key[(api.provider, api.interface)].append(api.name)
    names = {}
    for key, functions in by_key.items():
        first_words = collections.Counter(re.match(r"X[A-Z][a-z0-9]*", f).group(0) for f in functions)
        word = first_words.most_common(1)[0][0]
        family = [f for f in functions if f.startswith(word)]
        names[key] = os.path.commonprefix(family).rstrip("_") or word
    return names


def to_include(apis, source):
    lines = [
        "// Généré par tools/gdk_interfaces.py — ne pas modifier à la main.",
        f"// Source : {source}",
        "// ANYXBOXX_GDK_API(fonction, fournisseur, interface, n° de méthode)",
    ]
    lines += [f'ANYXBOXX_GDK_API({a.name}, "{a.provider}", "{a.interface}", {a.method})' for a in apis]
    return "\n".join(lines) + "\n"


def _version_key(path):
    """Tri numérique des dossiers de version : 14.10 après 14.9, 261000 après 251000."""
    return [int(part) if part.isdigit() else part for part in re.split(r"(\d+)", str(path))]


def _first(pattern):
    found = sorted(glob.glob(pattern), key=_version_key)
    return Path(found[-1]) if found else None


def disassemble(lib, dumpbin):
    result = subprocess.run([str(dumpbin), "/nologo", "/disasm", str(lib)], capture_output=True,
                            text=True, encoding="utf-8", errors="replace", check=True)
    return result.stdout


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--lib", type=Path, default=_first(GDK_LIBS))
    parser.add_argument("--dumpbin", type=Path, default=_first(DUMPBINS))
    parser.add_argument("--out", type=Path, default=DEFAULT_OUT)
    args = parser.parse_args(argv)
    if not args.lib or not args.lib.is_file():
        parser.error("xgameruntime.lib introuvable : installez le GDK public ou passez --lib.")
    if not args.dumpbin or not args.dumpbin.is_file():
        parser.error("dumpbin.exe introuvable : installez Visual Studio (C++) ou passez --dumpbin.")
    unreadable = []
    apis = parse(disassemble(args.lib, args.dumpbin), unreadable)
    if unreadable:
        print(f"Appels QueryApiImpl illisibles ({len(unreadable)}) : {', '.join(unreadable)}",
              file=sys.stderr)
        return 1
    source = f"{args.lib.parent.parent.parent.parent.name}/{args.lib.name}"   # version du GDK
    args.out.write_text(to_include(apis, source), encoding="utf-8", newline="\n")
    print(f"{len(apis)} API, {len(groups(apis))} interfaces → {args.out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
