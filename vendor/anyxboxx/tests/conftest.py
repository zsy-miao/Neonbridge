import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1] / "PrototypeScarlett"
sys.path.insert(0, str(Path(__file__).resolve().parent))
sys.path.insert(0, str(ROOT))

import os  # noqa: E402
import tempfile  # noqa: E402

import pytest  # noqa: E402

# Tests déterministes : AnyXboxX gère seul le runtime GDK, même si Gaming Services est
# installé. Les tests de délégation lancent un sous-processus avec la valeur "1".
os.environ.setdefault("ANYXBOXX_SYSTEM_RUNTIME", "0")
# Cache de shaders propre à la session de tests : jamais celui de l'utilisateur.
os.environ.setdefault("ANYXBOXX_SHADER_CACHE", tempfile.mkdtemp(prefix="anyxboxx-shader-cache-"))
# Sans DLL de TomBox dans les tests du runtime : ils vérifient nos DLL. Le dossier par défaut
# (runtime/build/tombox) est testé à part, et tests/test_tombox_native.py le désigne explicitement.
os.environ.setdefault("ANYXBOXX_TOMBOX_DIR", tempfile.mkdtemp(prefix="anyxboxx-sans-tombox-"))

RUNTIME_DIR = Path(os.environ.get("ANYXBOXX_RUNTIME")
                   or Path(__file__).resolve().parents[1] / "runtime" / "build" / "bin")
RUNTIME_DLLS = ("xgameruntime.dll", "xmem.dll", "d3d12_xs.dll", "kernelx.dll", "anyxboxx_tls.dll", "XCRDAPI.dll")


def runtime_built():
    return all((RUNTIME_DIR / name).is_file() for name in (*RUNTIME_DLLS, "anyxboxx.exe"))


@pytest.fixture
def runtime_dir():
    """Dossier du runtime C++ ; absent : test ignoré, sauf en CI (ANYXBOXX_REQUIRE_RUNTIME=1)."""
    if not runtime_built():
        if os.environ.get("ANYXBOXX_REQUIRE_RUNTIME") == "1":
            pytest.fail(f"Runtime C++ absent de {RUNTIME_DIR} : compilez runtime/ avec CMake.")
        pytest.skip("runtime C++ non compilé (cmake -S runtime -B runtime/build)")
    return RUNTIME_DIR
