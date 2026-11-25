"""Banc de performance : objectif 60 images par seconde constantes.

Le compteur du runtime C++ (runtime/xgameruntime/frame_counter.cpp) écrit une
ligne par seconde dans ANYXBOXX_FPS_CSV. Ce module lance le jeu en mode natif
pendant N secondes, relit ces mesures, rend un verdict et trace la courbe.
"""
import csv
import io
import os
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass
from pathlib import Path
from xml.sax.saxutils import escape

TARGET_FPS = 60.0
TOLERANCE = 0.5            # 59,5 i/s mesurés sur une seconde comptent comme 60
WARM_UP_SECONDS = 1        # la première seconde (chargement) est exclue du verdict
SCARLETT = Path(__file__).resolve().parents[1] / "scarlett.py"
WIDTH, HEIGHT, MARGIN = 760, 300, 48
GREEN, ORANGE, GRID, TEXT = "#2ea043", "#e8890c", "#d0d7de", "#1d2230"


@dataclass(frozen=True)
class Second:
    seconds: float
    frames: int
    fps: float
    average_ms: float
    p99_ms: float


@dataclass(frozen=True)
class Summary:
    seconds: int
    average_fps: float
    minimum_fps: float
    low_fps: float          # pire « 1 % bas » : 1000 / plus lent 99e centile mesuré
    constant_60: bool


def parse_csv(text):
    reader = csv.DictReader(io.StringIO(text))
    return tuple(Second(float(row["secondes"]), int(row["images"]), float(row["fps"]),
                        float(row["ms_moyen"]), float(row["ms_p99"])) for row in reader)


def summarize(rows):
    measured = rows[WARM_UP_SECONDS:]
    if not measured:
        raise ValueError("Mesure trop courte : aucune seconde après la chauffe, pas de verdict.")
    fps = [row.fps for row in measured]
    return Summary(len(measured), sum(fps) / len(fps), min(fps),
                   1000.0 / max(row.p99_ms for row in measured),
                   all(value >= TARGET_FPS - TOLERANCE for value in fps))


def text(summary):
    verdict = "oui" if summary.constant_60 else "non"
    return (f"Mesure sur {summary.seconds} s : moyenne {summary.average_fps:.1f} i/s · "
            f"minimum {summary.minimum_fps:.1f} i/s · 1 % bas {summary.low_fps:.1f} i/s · "
            f"60 i/s constants : {verdict}")


def svg(rows, title):
    """Courbe des images par seconde, avec la ligne d'objectif à 60 i/s."""
    top = max([TARGET_FPS * 1.25] + [row.fps for row in rows])
    span = max(row.seconds for row in rows) if rows else 1.0
    plot_w, plot_h = WIDTH - 2 * MARGIN, HEIGHT - 2 * MARGIN

    def x(seconds):
        return MARGIN + plot_w * seconds / span

    def y(fps):
        return HEIGHT - MARGIN - plot_h * fps / top

    points = " ".join(f"{x(row.seconds):.1f},{y(row.fps):.1f}" for row in rows)
    color = GREEN if len(rows) > WARM_UP_SECONDS and summarize(rows).constant_60 else ORANGE
    return "\n".join([
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{WIDTH}" height="{HEIGHT}" '
        f'viewBox="0 0 {WIDTH} {HEIGHT}" font-family="Segoe UI, Arial, sans-serif" font-size="12">',
        f'<rect width="{WIDTH}" height="{HEIGHT}" fill="#f7f8fa"/>',
        f'<text x="{MARGIN}" y="28" font-size="16" font-weight="600" fill="{TEXT}">'
        f'{escape(title)}: frames per second</text>',
        f'<line x1="{MARGIN}" y1="{y(TARGET_FPS):.1f}" x2="{WIDTH - MARGIN}" y2="{y(TARGET_FPS):.1f}" '
        f'stroke="{GRID}" stroke-width="2" stroke-dasharray="6 4"/>',
        f'<text x="{WIDTH - MARGIN}" y="{y(TARGET_FPS) - 6:.1f}" text-anchor="end" fill="{TEXT}">60 fps target</text>',
        f'<line x1="{MARGIN}" y1="{HEIGHT - MARGIN}" x2="{WIDTH - MARGIN}" y2="{HEIGHT - MARGIN}" stroke="{TEXT}"/>',
        f'<text x="{MARGIN}" y="{HEIGHT - MARGIN + 18}" fill="{TEXT}">0 s</text>',
        f'<text x="{WIDTH - MARGIN}" y="{HEIGHT - MARGIN + 18}" text-anchor="end" fill="{TEXT}">{span:.0f} s</text>',
        f'<polyline points="{points}" fill="none" stroke="{color}" stroke-width="3"/>',
        "</svg>",
    ])


def run(exe, seconds=10, chart=None, title=None, log=print):
    """Lance `exe` en mode natif pendant `seconds` secondes, mesure et résume."""
    exe = Path(exe)
    with tempfile.TemporaryDirectory() as folder:
        csv_path = Path(folder) / "fps.csv"
        environment = {**os.environ, "ANYXBOXX_FPS_CSV": str(csv_path)}
        process = subprocess.Popen([sys.executable, str(SCARLETT), "--natif", str(exe)], env=environment,
                                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        try:
            time.sleep(seconds)
        finally:
            process.kill()
            process.wait()
        if not csv_path.is_file():
            raise ValueError("Aucune mesure : le jeu n'a créé aucune swap chain DXGI.")
        rows = parse_csv(csv_path.read_text(encoding="utf-8"))
    summary = summarize(rows)
    log(text(summary))
    if chart is not None:
        Path(chart).write_text(svg(rows, title or exe.stem), encoding="utf-8")
    return summary
