"""Carte de l'avancée : treemaps SVG, une case par fonction (verte si traduite, grise sinon)."""
import importlib.util
import math
from dataclasses import dataclass
from pathlib import Path
from xml.sax.saxutils import escape

import pefile

from . import translation
from .report import analyse

REPOSITORY = Path(__file__).resolve().parents[2]
WIDTH, MARGIN, GAP = 960, 24, 4
BG, PANEL, GREEN, GREY, TEXT, ORANGE = "#0d1117", "#161b22", "#2ea043", "#484f58", "#e6edf3", "#d29922"
MUTED = "#8b949e"
BADGE_H, TITLE_H, BLOCK_PAD = 22, 34, 3
MIN_CELL, MAX_CELL, PITCH_AREA = 3, 10, 81      # taille d'une case, aire visée par fonction
MIN_SECTION_H = 110
GPU_FAMILIES = ("SOP1", "SOP2", "SOPK", "SOPC", "SOPP", "SMEM", "VOP1", "VOP2", "VOP3", "VOP3P",
                "VOPC", "DS", "MUBUF", "MTBUF", "MIMG", "FLAT", "GLOBAL", "SCRATCH", "EXP", "VINTRP")
GPU_TITLE = "RDNA 2 GPU instructions (reference interpreter)"
DISASSEMBLY_TITLE = "RDNA 2 GPU instructions (disassembler, LLVM syntax)"


@dataclass(frozen=True)
class Rect:
    """Rectangle : coin supérieur gauche (x, y), largeur w, hauteur h."""
    x: float
    y: float
    w: float
    h: float


@dataclass(frozen=True)
class Block:
    """Un rectangle du treemap : `done` fonctions traduites sur `total`."""
    name: str
    done: int
    total: int


@dataclass(frozen=True)
class Section:
    """Un treemap. `counted` faux : blocs sans cases ni chiffres, seulement une note."""
    title: str
    blocks: tuple
    counted: bool = True
    note: str = ""

    @property
    def done(self):
        return sum(b.done for b in self.blocks)

    @property
    def total(self):
        return sum(b.total for b in self.blocks)

    @property
    def heading(self):
        """Titre affiché : "GDK runtime: 17.9% (71/396)"."""
        if not self.counted:
            return f"{self.title}: 0% ({self.note})"
        share = 100 * self.done / self.total if self.total else 0.0
        return f"{self.title}: {share:.1f}% ({self.done}/{self.total})"


@dataclass(frozen=True)
class Status:
    """Données gelées de la carte : sections et badges (libellé, valeur, part de 0 à 1)."""
    sections: tuple
    badges: tuple


# --- Treemap « squarified » (Bruls, Huizing, van Wijk) -------------------------------------

def _worst(row, side):
    """Pire rapport d'aspect d'une rangée d'aires posée le long d'un côté `side`."""
    total = sum(row)
    return max(max(side * side * a / (total * total), total * total / (side * side * a)) for a in row)


def _place_row(row, rect):
    """Pose une rangée le long du côté court de `rect` ; rend (rectangles, rectangle restant)."""
    total = sum(a for _, a in row)
    placed, offset = [], 0.0
    if rect.w >= rect.h:                      # colonne à gauche
        width = total / rect.h
        for name, area in row:
            height = area / width
            placed.append((name, Rect(rect.x, rect.y + offset, width, height)))
            offset += height
        return placed, Rect(rect.x + width, rect.y, rect.w - width, rect.h)
    height = total / rect.w                   # ligne en haut
    for name, area in row:
        width = area / height
        placed.append((name, Rect(rect.x + offset, rect.y, width, height)))
        offset += width
    return placed, Rect(rect.x, rect.y + height, rect.w, rect.h - height)


def squarify(items, rect):
    """Treemap de `items` [(nom, poids)] dans `rect` : rectangles d'aire proportionnelle au poids."""
    kept = sorted(((n, w) for n, w in items if w > 0), key=lambda item: -item[1])
    if not kept:
        return ()
    scale = rect.w * rect.h / sum(w for _, w in kept)
    pending = [(n, w * scale) for n, w in kept]
    result, row, free = [], [], rect
    while pending:
        side = min(free.w, free.h)
        candidate = [*row, pending[0]]
        if not row or _worst([a for _, a in candidate], side) <= _worst([a for _, a in row], side):
            row = candidate
            pending = pending[1:]
            continue
        placed, free = _place_row(row, free)
        result += placed
        row = []
    placed, _ = _place_row(row, free)
    return (*result, *placed)


# --- Collecte ------------------------------------------------------------------------------

def _gdk_tools():
    """Charge tools/gdk_interfaces.py (hors du paquet Python) sans toucher à sys.path."""
    path = REPOSITORY / "tools" / "gdk_interfaces.py"
    spec = importlib.util.spec_from_file_location("anyxboxx_gdk_interfaces", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def gdk_blocks(table=translation.RUNTIME_SOURCES / "gdk_api_table.inc",
               served=translation.RUNTIME_SOURCES / "served_interfaces.inc", sources=translation.RUNTIME_SOURCES):
    """Un bloc par interface GDK, nommée par le préfixe commun de ses fonctions (stubs non comptés)."""
    tools = _gdk_tools()
    apis = tuple(tools.GdkApi(n, p, i, int(m)) for n, p, i, m in
                 translation.API_LINE.findall(Path(table).read_text(encoding="utf-8")))
    served_keys = {(p.upper(), i.upper()) for p, i in
                   translation.SERVED_LINE.findall(Path(served).read_text(encoding="utf-8"))}
    stubs = translation.not_implemented_functions(sources)
    names = tools.groups(apis)
    counts = {}
    for api in apis:
        done, total = counts.get((api.provider, api.interface), (0, 0))
        counts[(api.provider, api.interface)] = (
            done + ((api.provider, api.interface) in served_keys and api.name not in stubs), total + 1)
    return tuple(Block(names[key], d, t) for key, (d, t) in sorted(counts.items(), key=lambda kv: names[kv[0]]))


def package_blocks(package_dir):
    """Un bloc par module PE32+ du paquet : imports émulés ou fournis sur imports totaux."""
    blocks = []
    for path in translation._modules(package_dir):
        try:
            report = analyse(path)
        except (OSError, ValueError, pefile.PEFormatError):
            continue
        blocks.append(Block(path.name, len(report.emulated) + len(report.provided), report.total))
    return tuple(b for b in blocks if b.total)


def console_blocks(package_dir, runtime_dir, tombox_dir=None):
    """Un bloc par DLL console importée par le paquet (exports du runtime C++ et des DLL TomBox préférées)."""
    areas = translation.measure(package_dir, runtime_dir, tombox_dir=tombox_dir)
    return tuple(Block(a.name, a.done, a.total) for a in areas)


def gpu_blocks():
    """Un bloc par famille d'encodage RDNA 2 : encodages exécutés par l'interpréteur de référence."""
    from .rdna2_exec import executable
    from .rdna2_ops import MNEMONICS

    blocks = []
    for family in GPU_FAMILIES:
        names = [name for (key, _), name in MNEMONICS.items() if key == family]
        blocks.append(Block(family, sum(executable(family, n) for n in names), len(names)))
    return tuple(b for b in blocks if b.total)


def disassembly_blocks():
    """Un bloc par famille d'encodage RDNA 2 : encodages écrits avec leurs opérandes (syntaxe LLVM)."""
    from .rdna2_coverage import by_family

    counts = by_family()
    return tuple(Block(family, *counts[family]) for family in GPU_FAMILIES if family in counts)


def _badge(label, done, total):
    return (label, f"{done}/{total}", done / total if total else 0.0)


def collect(package_dir, runtime_dir, tombox_dir=None):
    """Mesure tout : sections de la carte et badges d'en-tête."""
    gdk = Section("GDK runtime", gdk_blocks())
    imports = Section("Package imports", package_blocks(package_dir))
    console = Section("Console DLLs", console_blocks(package_dir, runtime_dir, tombox_dir))
    disassembly = Section(DISASSEMBLY_TITLE, disassembly_blocks())
    gpu = Section(GPU_TITLE, gpu_blocks())
    badges = (("CPU x86-64", "100%", 1.0),
              _badge("GDK runtime", gdk.done, gdk.total),
              _badge("Package imports", imports.done, imports.total),
              _badge("Console DLLs", console.done, console.total),
              _badge("GPU disassembly", disassembly.done, disassembly.total),
              _badge("GPU RDNA 2", gpu.done, gpu.total))
    return Status((gdk, imports, console, disassembly, gpu), badges)


# --- Rendu SVG -----------------------------------------------------------------------------

def badge_color(share):
    """Vert à 100 %, orange s'il y a de l'avancée, gris sinon."""
    if share >= 1.0:
        return GREEN
    return ORANGE if share > 0 else GREY


def _badges(badges, top):
    parts, x = [], MARGIN
    for label, value, share in badges:
        left, right = 14 + 6.6 * len(label), 14 + 6.6 * len(value)
        parts.append(
            f'<g><title>{escape(label)}: {escape(value)}</title>'
            f'<rect x="{x:.1f}" y="{top}" width="{left + right:.1f}" height="{BADGE_H}" rx="4" fill="{PANEL}"/>'
            f'<rect x="{x + left:.1f}" y="{top}" width="{right:.1f}" height="{BADGE_H}" rx="4" '
            f'fill="{badge_color(share)}"/>'
            f'<text x="{x + left / 2:.1f}" y="{top + 15}" text-anchor="middle" fill="{TEXT}" font-size="12">'
            f'{escape(label)}</text>'
            f'<text x="{x + left + right / 2:.1f}" y="{top + 15}" text-anchor="middle" fill="#ffffff" '
            f'font-size="12" font-weight="600">{escape(value)}</text></g>')
        x += left + right + 8
    return parts


def _truncate(text, width, size=11):
    """Titre raccourci (…) pour tenir dans `width` pixels."""
    fit = int((width - 8) / (size * 0.58))
    if fit < 3:
        return ""
    return text if len(text) <= fit else text[:fit - 1] + "…"


def _grid_size(count, width, height):
    """Plus grande case (3 à 10 px, 1 px d'espace) dont la grille contient `count` cases."""
    for size in range(MAX_CELL, MIN_CELL - 1, -1):
        pitch = size + 1
        if (int(width) // pitch) * (int(height) // pitch) >= count:
            return size
    return MIN_CELL


def _cells(block, rect, top):
    """Cases d'un bloc : vertes d'abord, puis grises, sous la zone de titre."""
    inner_w, inner_h = rect.w - 2 * BLOCK_PAD, rect.h - top - BLOCK_PAD
    if block.total <= 0 or inner_w < MIN_CELL or inner_h < MIN_CELL:
        return []
    size = _grid_size(block.total, inner_w, inner_h)
    pitch, columns = size + 1, max(1, int(inner_w) // (size + 1))
    cells = []
    for index in range(block.total):
        row, column = divmod(index, columns)
        css = "ok" if index < block.done else "no"
        cells.append(f'<rect class="{css}" x="{rect.x + BLOCK_PAD + column * pitch:.1f}" '
                     f'y="{rect.y + top + row * pitch:.1f}" width="{size}" height="{size}"/>')
    return cells


def _block_svg(block, rect, counted):
    """Groupe SVG d'un bloc : panneau, titre tronqué, cases, infobulle."""
    tip = f"{block.name}: {block.done}/{block.total}" if counted else block.name
    header = 16 if rect.w >= 40 and rect.h >= 30 else 0
    parts = [f'<g><title>{escape(tip)}</title>',
             f'<rect x="{rect.x:.1f}" y="{rect.y:.1f}" width="{rect.w:.1f}" height="{rect.h:.1f}" '
             f'rx="2" fill="{PANEL}" stroke="{BG}" stroke-width="1"/>']
    if header and _truncate(block.name, rect.w):
        parts.append(f'<text x="{rect.x + 4:.1f}" y="{rect.y + 12:.1f}" font-size="11" fill="{TEXT}">'
                     f'{escape(_truncate(block.name, rect.w))}</text>')
    if counted:
        parts += _cells(block, rect, header or BLOCK_PAD)
    return parts + ["</g>"]


def _section_height(section):
    cells = section.total if section.counted else len(section.blocks) * 12
    return max(MIN_SECTION_H, math.ceil(cells * PITCH_AREA * 1.5 / (WIDTH - 2 * MARGIN)))


def _section_svg(section, top):
    """Titre puis treemap de la section ; rend (éléments SVG, hauteur utilisée)."""
    height = _section_height(section)
    area = Rect(MARGIN, top + TITLE_H, WIDTH - 2 * MARGIN, height)
    weights = [(b.name, b.total if section.counted else 1) for b in section.blocks]
    by_name = {b.name: b for b in section.blocks}
    parts = [f'<text x="{MARGIN}" y="{top + 20}" font-size="15" font-weight="600" fill="{TEXT}">'
             f'{escape(section.heading)}</text>']
    for name, rect in squarify(weights, area):
        parts += _block_svg(by_name[name], rect, section.counted)
    return parts, TITLE_H + height + 22


def render(status):
    """Image SVG autonome (960 px de large) de la carte de l'avancée."""
    body, top = [], MARGIN + BADGE_H + 18
    body += _badges(status.badges, MARGIN)
    for section in status.sections:
        if not section.blocks or (section.counted and not section.total):
            continue
        parts, used = _section_svg(section, top)
        body += parts
        top += used
    height = top + MARGIN - 10
    style = (f'<style>.ok{{fill:{GREEN}}}.no{{fill:{GREY}}}</style>')
    head = (f'<svg xmlns="http://www.w3.org/2000/svg" width="{WIDTH}" height="{height}" '
            f'viewBox="0 0 {WIDTH} {height}" font-family="Segoe UI, Arial, sans-serif" font-size="13">')
    return "\n".join([head, style, f'<rect width="{WIDTH}" height="{height}" fill="{BG}"/>', *body, "</svg>"])
