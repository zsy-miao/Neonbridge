"""Site GitHub Pages (site/index.html) : structure et estimation matérielle."""
from pathlib import Path

import pytest

RACINE = Path(__file__).resolve().parents[1]
SITE = RACINE / "site" / "index.html"


def test_site_states_the_game_is_not_playable():
    html = SITE.read_text(encoding="utf-8")
    assert "Not yet." in html and "game_scarlett_master.exe" in html
    assert "leonida/progress.html" not in html


def test_hardware_estimate_logic_passes_its_node_tests():
    import shutil
    import subprocess
    node = shutil.which("node")
    if node is None:
        pytest.skip("node absent : la logique de site/estimate.js se teste avec node --test")
    resultat = subprocess.run([node, "--test", str(RACINE / "site" / "estimate.test.mjs")],
                              capture_output=True, text=True, encoding="utf-8", timeout=120)
    assert resultat.returncode == 0, resultat.stdout[-2000:] + resultat.stderr[-2000:]
