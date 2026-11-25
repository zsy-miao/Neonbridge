"""tools/build_tombox.py affiche la progression de cargo en direct et garde sa sortie d'erreur
pour y trouver les fichiers manquants (toutes plateformes : un faux cargo en Python suffit)."""
import importlib.util
import io
import subprocess
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
WAIT_SECONDS = 10

# Faux cargo : écrit une ligne de progression, puis attend que le test l'ait reçue avant de finir.
# Si la sortie n'était relue qu'à la fin, le signal n'arriverait jamais et il échouerait.
FAKE_CARGO = """
import sys, time
from pathlib import Path
signal = Path(sys.argv[1])
sys.stderr.write("   Compiling compat-common v0.1.0\\n")
sys.stderr.flush()
deadline = time.monotonic() + {wait}
while not signal.exists() and time.monotonic() < deadline:
    time.sleep(0.05)
if not signal.exists():
    sys.stderr.write("progression non reçue en direct\\n")
    sys.exit(2)
sys.stderr.write("error: couldn't read `backend_data/route.json`\\n")
sys.exit(101)
""".format(wait=WAIT_SECONDS)


def build_script():
    spec = importlib.util.spec_from_file_location("build_tombox", ROOT / "tools" / "build_tombox.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class SignallingSink(io.StringIO):
    """Journal de test : crée `signal` dès la première ligne reçue."""

    def __init__(self, signal):
        super().__init__()
        self.signal = signal

    def write(self, text):
        self.signal.touch()
        return super().write(text)


def test_cargo_progress_is_shown_live_and_kept_for_missing_files(tmp_path):
    script = build_script()
    signal = tmp_path / "progression-recue"
    sink = SignallingSink(signal)
    result = script._run([sys.executable, "-c", FAKE_CARGO, str(signal)], tmp_path, sink)
    assert result.returncode == 101, result.stderr
    assert "Compiling compat-common" in sink.getvalue()
    assert sink.getvalue() == result.stderr
    assert script.MISSING.findall(result.stderr) == ["backend_data/route.json"]


def test_cargo_still_stops_at_the_timeout(tmp_path, monkeypatch):
    script = build_script()
    monkeypatch.setattr(script, "TIMEOUT", 0.5)
    with pytest.raises(subprocess.TimeoutExpired):
        script._run([sys.executable, "-c", "import time; time.sleep(30)"], tmp_path, io.StringIO())
