"""Carte de l'avancée : treemap, rendu SVG et collecte des données."""
import xml.etree.ElementTree as ET

import pytest

from emu import status_map
from emu.status_map import Block, Rect, Section, Status
from pe_builder import Asm, build_pe

CODE = Asm().mov_eax(1).ret().build()
AREA = Rect(0, 0, 900, 300)


def _overlap(a, b):
    return min(a.x + a.w, b.x + b.w) - max(a.x, b.x) > 1e-6 and \
        min(a.y + a.h, b.y + b.h) - max(a.y, b.y) > 1e-6


def test_treemap_covers_rectangle_without_overlap():
    items = [("a", 6), ("b", 6), ("c", 4), ("d", 3), ("e", 2), ("f", 2), ("g", 1)]
    placed = status_map.squarify(items, AREA)
    assert len(placed) == len(items)
    assert sum(r.w * r.h for _, r in placed) == pytest.approx(AREA.w * AREA.h, rel=0.01)
    rects = [r for _, r in placed]
    assert not any(_overlap(a, b) for i, a in enumerate(rects) for b in rects[i + 1:])
    assert all(r.x >= -1e-6 and r.y >= -1e-6 and r.x + r.w <= AREA.w + 1e-6
               and r.y + r.h <= AREA.h + 1e-6 for r in rects)


def test_treemap_areas_are_proportional_to_weights():
    items = [("a", 10), ("b", 5), ("c", 1)]
    areas = {n: r.w * r.h for n, r in status_map.squarify(items, AREA)}
    total = sum(areas.values())
    for name, weight in items:
        assert areas[name] / total == pytest.approx(weight / 16, rel=0.01)


def test_treemap_edge_cases():
    assert status_map.squarify([], AREA) == ()
    assert status_map.squarify([("a", 0)], AREA) == ()
    (only,) = status_map.squarify([("a", 3)], AREA)
    assert only[1] == AREA


def test_treemap_does_not_mutate_input():
    items = [("a", 1), ("b", 5)]
    status_map.squarify(items, AREA)
    assert items == [("a", 1), ("b", 5)]


def _status():
    return Status(
        sections=(
            Section("GDK runtime", (Block("XTaskQueue", 3, 5), Block("XUser", 0, 4))),
            Section("DLL <console>", (Block("xmem.dll", 1, 2),)),
            Section("GPU", (Block("SOP1", 0, 0), Block("VOP3", 0, 0)), counted=False,
                    note="recompiler not started"),
        ),
        badges=(("CPU x86-64", "100 %", 1.0), ("GPU RDNA 2", "0 %", 0.0),
                ("Package imports", "1/4", 0.25)))


def test_render_is_valid_xml_with_one_green_cell_per_translated_function():
    image = status_map.render(_status())
    root = ET.fromstring(image)
    classes = [e.get("class") for e in root.iter() if e.tag.endswith("rect")]
    assert classes.count("ok") == 4
    assert classes.count("no") == (5 - 3) + 4 + 1
    assert root.get("width") == "960"
    assert "DLL &lt;console&gt;" in image
    assert "<script" not in image


def test_render_tooltips_and_titles():
    root = ET.fromstring(status_map.render(_status()))
    tips = [t.text for t in root.iter() if t.tag.endswith("title")]
    assert "XTaskQueue: 3/5" in tips
    text = "".join(root.itertext())
    assert "GDK runtime: 33.3% (3/9)" in text
    assert "GPU: 0% (recompiler not started)" in text


def test_badge_colors():
    assert status_map.badge_color(1.0) == status_map.GREEN
    assert status_map.badge_color(0.3) == status_map.ORANGE
    assert status_map.badge_color(0.0) == status_map.GREY


@pytest.fixture
def folders(tmp_path):
    package, runtime = tmp_path / "package", tmp_path / "runtime"
    package.mkdir()
    runtime.mkdir()
    build_pe(package / "game.exe", lambda iat: CODE, dll=False, imports=[
        ("xmem.dll", "XMemAlloc"), ("xmem.dll", "XMemFree"),
        ("kernel32.dll", "GetTickCount")])
    build_pe(runtime / "xmem.dll", lambda iat: CODE, exports=[("XMemAlloc", 0)])
    return package, runtime


def test_collect_small_package(folders):
    status = status_map.collect(*folders)
    titles = [s.title for s in status.sections]
    assert titles[0].startswith("GDK runtime")
    assert any(t.startswith("Package imports") for t in titles)
    console = next(s for s in status.sections if s.title.startswith("Console DLLs"))
    assert [(b.name, b.done, b.total) for b in console.blocks] == [("xmem.dll", 1, 2)]
    imports = next(s for s in status.sections if s.title.startswith("Package imports"))
    assert [b.name for b in imports.blocks] == ["game.exe"]
    assert imports.blocks[0].total == 3
    gpu = status.sections[-1]
    assert gpu.counted and 0 < gpu.done < gpu.total
    done = {b.name: b.done for b in gpu.blocks}
    assert done["VOP3"] > 0 and done["DS"] > 0 and done["MIMG"] >= 1     # image_sample exécuté
    assert dict((n, v) for n, v, _ in status.badges)["GPU RDNA 2"] == f"{gpu.done}/{gpu.total}"
    ET.fromstring(status_map.render(status))


def test_collect_without_runtime_dir_leaves_console_cells_grey(folders):
    status = status_map.collect(folders[0], None)
    console = next(s for s in status.sections if s.title.startswith("Console DLLs"))
    assert console.done == 0 and console.total == 2
