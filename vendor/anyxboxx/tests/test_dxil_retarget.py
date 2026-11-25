"""Retarget du DXIL console (dxil-ms-xb2) vers le DXIL du PC (dxil-ms-dx), outil de Xbox2Windows.

Le code et son test viennent de Xbox2Windows (MIT), copiés dans third_party/xbox2windows/ : leur
test fabrique ses propres modules DXIL et vérifie triple, métadonnées et conteneur réécrits.
"""
import subprocess
import sys
from pathlib import Path

X2W = Path(__file__).resolve().parents[1] / "third_party" / "xbox2windows"
TIMEOUT = 300


def test_upstream_retarget_check_passes():
    result = subprocess.run([sys.executable, "-B", str(X2W / "tests" / "dxil_retarget_check.py")],
                            capture_output=True, text=True, timeout=TIMEOUT)
    assert result.returncode == 0, result.stdout + result.stderr
    assert "dxil_retarget controls: ok" in result.stdout


def test_command_line_documents_the_desktop_triple():
    result = subprocess.run([sys.executable, "-B", str(X2W / "tools" / "dxil_retarget.py"), "--help"],
                            capture_output=True, text=True, timeout=TIMEOUT)
    assert result.returncode == 0 and "dxil-ms-dx" in result.stdout
