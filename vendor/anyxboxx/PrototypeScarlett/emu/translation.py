"""Avancée de la « traduction » des DLL propres à la console Xbox Series X."""
import re
from dataclasses import dataclass
from pathlib import Path
from xml.sax.saxutils import escape

import pefile

from .bench import PATTERNS, is_pe32_plus
from .modules import dll_name
from .tombox_paths import TOMBOX_INSTALL, TOMBOX_PREFERRED

CONSOLE_DLLS = (
    "xmem.dll", "d3d12_xs.dll", "kernelx.dll", "xg_xs.dll", "xgameruntime.dll",
    "xcrdapi.dll", "xapu.dll", "acphal.dll", "chad.dll",
)
TOMBOX_BUILD = TOMBOX_INSTALL      # dossier des DLL de TomBox par défaut (tools/build_tombox.py)
DIRECTORY_IMPORT = pefile.DIRECTORY_ENTRY["IMAGE_DIRECTORY_ENTRY_IMPORT"]
DIRECTORY_EXPORT = pefile.DIRECTORY_ENTRY["IMAGE_DIRECTORY_ENTRY_EXPORT"]
COMPLETE = "complete"
MAX_LISTED = 4
WIDTH, MARGIN, LABEL_W, BAR_X, BAR_W = 760, 24, 270, 300, 220
HEADER_H, ROW_H = 76, 46
GREEN, ORANGE, RED, TRACK = "#107c10", "#e8890c", "#c42b1c", "#e3e6ee"


@dataclass(frozen=True)
class Area:
    """Un domaine à traduire : `done` éléments faits sur `total`."""
    name: str
    done: int
    total: int
    note: str
    counted: bool = True   # False : seule la part est affichée (domaine non dénombrable)

    @property
    def share(self):
        """Part traduite entre 0 et 1 (0 si rien à traduire)."""
        return self.done / self.total if self.total else 0.0


# Exportées par le runtime mais seulement amorcées : elles signalent un échec propre.
PARTIAL = {
    "XMemGetWorkingSetStatistics": "non-public structure",
}

CPU_AREA = Area("CPU instructions (x86-64, Zen 2)", 1, 1,
                "same instruction set as the PC: run as-is in native mode",
                counted=False)
def gpu_area():
    """Instructions RDNA 2 exécutées par l'interpréteur de référence (première étape du recompilateur)."""
    from .rdna2_exec import coverage

    done, total = coverage()
    return Area("GPU instructions (RDNA 2, console shaders)", done, total,
                "run by the reference interpreter (wave32); recompiled to DXIL: compute, vertex, pixel")


def gpu_disassembly_area():
    """Encodages RDNA 2 relus et écrits avec leurs opérandes par le désassembleur (syntaxe LLVM)."""
    from .rdna2_coverage import coverage

    done, total = coverage()
    return Area("GPU instructions disassembled (RDNA 2, LLVM syntax)", done, total,
                "operands checked against LLVM's public encoding tests, SDWA and DPP forms included")

STATIC_AREAS = (
    Area("D3D12.X: device and ID3D12* interfaces", 1, 1,
         "served by TomBox's d3d12_xs.dll (194/200 methods by their count), not yet tried with a game",
         counted=False),
)


RUNTIME_SOURCES = Path(__file__).resolve().parents[2] / "runtime" / "xgameruntime"
API_LINE = re.compile(r'ANYXBOXX_GDK_API\((\w+), "([^"]+)", "([^"]+)", (\d+)\)')
SERVED_LINE = re.compile(r'ANYXBOXX_SERVED_INTERFACE\(\w+, "([^"]+)", "([^"]+)"\)')
# Méthode d'une table C++ branchée sur le stub, avec le nom de la fonction en commentaire.
NOT_IMPLEMENTED_LINE = re.compile(r'ANYXBOXX_NOT_IMPLEMENTED,\s*//\s*\d+\s+(X\w+)')


def not_implemented_functions(sources=RUNTIME_SOURCES):
    """Fonctions X* que les tables C++ du runtime branchent sur ANYXBOXX_NOT_IMPLEMENTED."""
    return {name for path in Path(sources).glob("*.cpp")
            for name in NOT_IMPLEMENTED_LINE.findall(path.read_text(encoding="utf-8"))}


def gdk_runtime_area(table=RUNTIME_SOURCES / "gdk_api_table.inc",
                     served=RUNTIME_SOURCES / "served_interfaces.inc", sources=RUNTIME_SOURCES):
    """Fonctions X* du GDK écrites par AnyXboxX : interface servie (et non par Windows), sans stub."""
    apis = API_LINE.findall(Path(table).read_text(encoding="utf-8"))
    stubs = not_implemented_functions(sources)
    served_keys = set((p.upper(), i.upper()) for p, i in SERVED_LINE.findall(Path(served).read_text(encoding="utf-8")))
    interfaces = {(provider, iid) for _, provider, iid, _ in apis}
    done = sum((provider, iid) in served_keys and name not in stubs for name, provider, iid, _ in apis)
    return Area("GDK runtime functions served by AnyXboxX", done, len(apis),
                f"{len(served_keys & interfaces)}/{len(interfaces)} interfaces (used when the Windows "
                "GDK runtime is off)")


def functions_area(rows):
    """Fonctions système importées par le paquet et résolues en mode isolé (banc d'essai)."""
    total = sum(r.imports for r in rows)
    done = sum(round(r.imports * r.coverage / 100) for r in rows)
    return Area("System functions imported by the package", done, total,
                "isolated mode: emulated or provided by a package DLL")


def _modules(folder):
    paths = {p for pattern in PATTERNS for p in Path(folder).glob(pattern)}
    return sorted((p for p in paths if p.is_file() and is_pe32_plus(p)),
                  key=lambda p: p.name.lower())


def _imported_names(path):
    """Imports du module : {dll normalisée: noms}, ordinaux écrits `#n`."""
    try:
        pe = pefile.PE(str(path), fast_load=True)
    except (OSError, pefile.PEFormatError):
        return {}
    try:
        pe.parse_data_directories(directories=[DIRECTORY_IMPORT])
        found = {}
        for entry in getattr(pe, "DIRECTORY_ENTRY_IMPORT", ()):
            dll = dll_name(entry.dll.decode("latin-1"))
            for imp in entry.imports:
                name = imp.name.decode("latin-1") if imp.name else f"#{imp.ordinal}"
                found.setdefault(dll, set()).add(name)
        return found
    except (pefile.PEFormatError, ValueError):
        return {}
    finally:
        pe.close()


def package_imports(folder):
    """Fonctions importées par le paquet, regroupées par DLL console."""
    merged = {}
    for path in _modules(folder):
        for dll, names in _imported_names(path).items():
            if dll in CONSOLE_DLLS:
                merged[dll] = merged.get(dll, frozenset()) | frozenset(names)
    return merged


def _exported_names(path):
    try:
        pe = pefile.PE(str(path), fast_load=True)
    except (OSError, pefile.PEFormatError):
        return frozenset()
    try:
        pe.parse_data_directories(directories=[DIRECTORY_EXPORT])
        directory = getattr(pe, "DIRECTORY_ENTRY_EXPORT", None)
        symbols = directory.symbols if directory else ()
        return frozenset(s.name.decode("latin-1") for s in symbols if s.name)
    except (pefile.PEFormatError, ValueError):
        return frozenset()
    finally:
        pe.close()


def runtime_exports(runtime_dir):
    """Exports nommés (redirections comprises) des DLL console présentes dans le runtime."""
    if runtime_dir is None or not Path(runtime_dir).is_dir():
        return {}
    result = {}
    for name in CONSOLE_DLLS:
        path = Path(runtime_dir) / name
        if path.is_file():
            result[name] = _exported_names(path)
    return result


def tombox_exports(tombox_dir):
    """Exports des DLL de TomBox que le lanceur préfère aux nôtres (TOMBOX_PREFERRED), si compilées."""
    if tombox_dir is None or not Path(tombox_dir).is_dir():
        return {}
    return {name: _exported_names(Path(tombox_dir) / name) for name in TOMBOX_PREFERRED
            if (Path(tombox_dir) / name).is_file()}


def measure(package_dir, runtime_dir, extra=(), tombox_dir=None):
    """Une `Area` par DLL console importée par le paquet, suivie de `extra`.

    `tombox_dir` : dossier des DLL de TomBox ; leurs exports comptent aussi (le lanceur les charge)."""
    imports = package_imports(package_dir)
    exports = runtime_exports(runtime_dir)
    tombox = tombox_exports(tombox_dir)
    areas = []
    for dll in CONSOLE_DLLS:
        if dll not in imports:
            continue
        exported = exports.get(dll, frozenset()) | tombox.get(dll, frozenset())
        missing = sorted(imports[dll] - exported)
        partial = sorted(n for n in imports[dll] & exported if n in PARTIAL and n not in tombox.get(dll, ()))
        remaining = missing + [f"{n} (partial: {PARTIAL[n]})" for n in partial]
        listed = ", ".join(remaining[:MAX_LISTED]) + (", …" if len(remaining) > MAX_LISTED else "")
        areas.append(Area(dll, len(imports[dll]) - len(remaining), len(imports[dll]),
                          listed if remaining else COMPLETE))
    return (*areas, *extra)


def table(areas):
    """Tableau Markdown de l'avancée."""
    lines = ["| Domaine | Traduit | Total | Part | Reste |", "|---|---|---|---|---|"]
    lines += [f"| {a.name} | {a.done} | {a.total} | {a.share * 100:.0f} % | {a.note} |"
              for a in areas]
    return "\n".join(lines)


def _color(area):
    if area.total and area.done >= area.total:
        return GREEN
    return ORANGE if area.done else RED


def _amount(area):
    share = f"{area.share * 100:.0f}%"
    return f"{area.done}/{area.total} ({share})" if area.counted else share


def _row(area, top):
    mid = top + 14
    fill = round(BAR_W * area.share)
    return [
        f'<text x="{LABEL_W}" y="{mid}" text-anchor="end" fill="#1d2230">{escape(area.name)}</text>',
        f'<rect x="{BAR_X}" y="{top}" width="{BAR_W}" height="18" rx="3" fill="{TRACK}"/>',
        f'<rect x="{BAR_X}" y="{top}" width="{fill}" height="18" rx="3" fill="{_color(area)}"/>',
        f'<text x="{BAR_X + BAR_W + 12}" y="{mid}" fill="#1d2230">'
        f'{_amount(area)}</text>',
        f'<text x="{BAR_X}" y="{top + 34}" font-size="11" fill="#4b5263">{escape(area.note)}</text>',
    ]


def svg(areas, title):
    """Image SVG autonome de l'avancée, une barre par domaine."""
    height = HEADER_H + ROW_H * len(areas) + 12
    body = [f'<svg xmlns="http://www.w3.org/2000/svg" width="{WIDTH}" height="{height}" '
            f'viewBox="0 0 {WIDTH} {height}" font-family="Segoe UI, Arial, sans-serif" '
            f'font-size="13">',
            f'<rect width="{WIDTH}" height="{height}" fill="#f7f8fa"/>',
            f'<text x="{MARGIN}" y="34" font-size="18" font-weight="600" fill="#1d2230">'
            f'{escape(title)}</text>',
            f'<text x="{MARGIN}" y="56" fill="#4b5263">Share translated for the Xbox Series X · '
            f'green complete, orange partial, red not started</text>']
    for index, area in enumerate(areas):
        body += _row(area, HEADER_H + ROW_H * index)
    return "\n".join([*body, "</svg>"])
