"""Exécution sur le GPU du PC d'un programme RDNA 2 recompilé : HLSL → DXIL (dxc) → D3D12.

    hlsl = rdna2_hlsl.translate(programme, sgprs)
    mémoire_finale = rdna2_gpu.run(hlsl, mémoire_initiale)

dxc vient du SDK Windows (ou de ANYXBOXX_DXC) et le DXIL est gardé en cache (emu/shader_cache.py) ;
anyxboxx_gpu_run.exe est compilé avec le runtime C++ (dossier ANYXBOXX_RUNTIME, sinon runtime/build/bin).
Sans GPU, WARP est utilisé.
"""
import os
import subprocess
import tempfile
from pathlib import Path

from . import shader_cache

REPOSITORY = Path(__file__).resolve().parents[2]
KITS = Path(os.environ.get("ProgramFiles(x86)", r"C:\Program Files (x86)")) / "Windows Kits" / "10" / "bin"
SHADER_MODEL = "cs_6_0"
VERTEX_MODEL, PIXEL_MODEL, PIXEL_WAVE_MODEL = "vs_6_0", "ps_6_0", "ps_6_6"
TIMEOUT = 120


_CACHE = shader_cache.ShaderCache()     # dossier créé à la première écriture


class GpuUnavailable(RuntimeError):
    """dxc ou le lanceur GPU est introuvable."""


def find_dxc():
    configured = os.environ.get("ANYXBOXX_DXC")
    if configured:
        return Path(configured) if Path(configured).is_file() else None
    found = sorted(KITS.glob("10.*/x64/dxc.exe"), reverse=True)
    return found[0] if found else None


def find_runner():
    folder = Path(os.environ.get("ANYXBOXX_RUNTIME") or REPOSITORY / "runtime" / "build" / "bin")
    runner = folder / "anyxboxx_gpu_run.exe"
    return runner if runner.is_file() else None


def adapter():
    """Champs de `anyxboxx_gpu_run.exe --adaptateur` pour l'adaptateur choisi par ANYXBOXX_GPU."""
    runner = find_runner()
    if runner is None:
        raise GpuUnavailable("anyxboxx_gpu_run.exe introuvable : compilez runtime/ avec CMake")
    result = subprocess.run([str(runner), "--adaptateur"], capture_output=True, encoding="utf-8",
                            errors="replace", timeout=TIMEOUT)
    if result.returncode != 0:
        raise GpuUnavailable(f"adaptateur introuvable :\n{result.stderr}")
    return dict(line.split("=", 1) for line in result.stdout.splitlines() if "=" in line)


def supports_doubles():
    """Vrai si l'adaptateur exécute des doubles dans les shaders (certains GPU intégrés ne le font pas)."""
    return adapter().get("double_precision") == "oui"


def available():
    return find_dxc() is not None and find_runner() is not None


def compile_hlsl(hlsl, folder, model=SHADER_MODEL):
    dxc = find_dxc()
    if dxc is None:
        raise GpuUnavailable("dxc introuvable : installez le SDK Windows ou définissez ANYXBOXX_DXC")
    source, output = Path(folder) / "shader.hlsl", Path(folder) / "shader.dxil"
    source.write_text(hlsl, encoding="utf-8")
    result = subprocess.run([str(dxc), "-nologo", "-T", model, "-E", "main", "-Fo", str(output), str(source)],
                            capture_output=True, text=True, timeout=TIMEOUT)
    if result.returncode != 0:
        raise RuntimeError(f"dxc a refusé le HLSL généré :\n{result.stderr}")
    return output


def compile_dxil(hlsl, cache=None, model=SHADER_MODEL):
    """DXIL de `hlsl` : depuis le cache disque si possible, sinon compilé par dxc puis mis en cache."""
    cache = cache if cache is not None else _CACHE
    key = shader_cache.cache_key(hlsl, model, find_dxc())
    cached = cache.get(key)
    if cached is not None:
        return cached
    with tempfile.TemporaryDirectory(prefix="anyxboxx-dxc-") as folder:
        dxil = compile_hlsl(hlsl, folder, model).read_bytes()
    cache.put(key, dxil)
    return dxil


def draw(vertex_hlsl, pixel_hlsl, memory, vertices, width, height, cache=None):
    """Dessine `vertices` sommets (liste de triangles) avec le VS et le PS (None : sans PS) sur une
    cible width × height ; u0 = `memory` (bytes). Renvoie la mémoire finale."""
    runner = find_runner()
    if runner is None:
        raise GpuUnavailable("anyxboxx_gpu_run.exe introuvable : compilez runtime/ avec CMake")
    with tempfile.TemporaryDirectory(prefix="anyxboxx-draw-") as folder:
        vs, ps, image = Path(folder) / "vs.dxil", Path(folder) / "ps.dxil", Path(folder) / "memory.bin"
        vs.write_bytes(compile_dxil(vertex_hlsl, cache, VERTEX_MODEL))
        if pixel_hlsl is not None:
            ps_model = PIXEL_WAVE_MODEL if "IsHelperLane()" in pixel_hlsl else PIXEL_MODEL
            ps.write_bytes(compile_dxil(pixel_hlsl, cache, ps_model))
        image.write_bytes(memory)
        arguments = [str(runner), "--draw", str(vs), str(ps) if pixel_hlsl is not None else "-", str(image),
                     str(vertices), str(width), str(height)]
        result = subprocess.run(arguments, capture_output=True, text=True, timeout=TIMEOUT)
        if result.returncode != 0:
            raise RuntimeError(f"dessin GPU impossible :\n{result.stderr}")
        return image.read_bytes()


def run(hlsl, memory, groups=1, cache=None):
    """Compile `hlsl` (cache disque), l'exécute avec `memory` (bytes) en u0 et renvoie la mémoire finale."""
    runner = find_runner()
    if runner is None:
        raise GpuUnavailable("anyxboxx_gpu_run.exe introuvable : compilez runtime/ avec CMake")
    dxil = compile_dxil(hlsl, cache)
    with tempfile.TemporaryDirectory(prefix="anyxboxx-gpu-") as folder:
        shader = Path(folder) / "shader.dxil"
        shader.write_bytes(dxil)
        image = Path(folder) / "memory.bin"
        image.write_bytes(memory)
        result = subprocess.run([str(runner), str(shader), str(image), str(groups)],
                                capture_output=True, text=True, timeout=TIMEOUT)
        if result.returncode != 0:
            raise RuntimeError(f"exécution GPU impossible :\n{result.stderr}")
        return image.read_bytes()
