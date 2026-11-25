"""Build and execute an app-local D3D12 control in a disposable, offline Proton prefix."""
from pathlib import Path
import shutil
import subprocess

from build import ROOT, Toolchain
from project_tuxbox.runtime_shaders import shader_session
from project_tuxbox.shader_prep import SOURCE_CACHE


def run_gpu_guest(source, name, trial, work, proton, prefix_source, provider, compiler_dir, *, vkd3d_dir=None, link_libraries=(), extra_env=None, fresh_shader_cache=False, warm_shader_cache=None):
    files = proton / "files"
    tools = Toolchain()
    obj = tools.compile(source, trial, "/W4", "/WX", f"/I{ROOT / 'runtime/d3d12'}")
    exe = trial / name
    tools.link(exe, obj, "d3d12.lib", "dxgi.lib", "kernel32.lib", *link_libraries)
    prefix = trial / "prefix"
    subprocess.run(["cp", "-a", "--reflink=auto", prefix_source.resolve(), prefix], check=True)
    if fresh_shader_cache or warm_shader_cache is not None:
        # Only touch the disposable prefix. Never traverse a linked cache.
        shader_cache = prefix / SOURCE_CACHE.parent
        if not shader_cache.resolve().is_relative_to(prefix.resolve()):
            raise ValueError("shader cache is outside the disposable prefix")
        if shader_cache.exists():
            shutil.rmtree(shader_cache)
        if warm_shader_cache is not None:
            shutil.copytree(warm_shader_cache, shader_cache / "runtime")
    shutil.copyfile(provider, trial / "d3d12_xs.dll")
    for lib in ("dxcompiler.dll", "dxil.dll"):
        shutil.copyfile(compiler_dir / lib, trial / lib)
    wine_libs = files / "lib/wine"
    vkd3d_dir = vkd3d_dir or wine_libs / "vkd3d-proton/x86_64-windows"
    for lib in ("d3d12.dll", "d3d12core.dll"):
        shutil.copyfile(vkd3d_dir / lib, trial / lib)
    shutil.copyfile(wine_libs / "dxvk/x86_64-windows/dxgi.dll", trial / "dxgi.dll")
    for name in ("tmp", "runtime", "cache"):
        (trial / name).mkdir(mode=0o700)
    # Terminate the disposable Wine server before the caller removes its prefix.
    command = ["bwrap", "--die-with-parent", "--unshare-pid", "--unshare-net", "--ro-bind", "/", "/", "--dev-bind", "/dev",
               "/dev", "--proc", "/proc", "--tmpfs", "/tmp", "--bind", str(trial), str(trial),
               "--unsetenv", "WAYLAND_DISPLAY", "--chdir", str(trial)]
    environment = dict(WINEPREFIX=prefix, WINESERVER=files / "bin/wineserver", WINEDEBUG="-all", VKD3D_DEBUG="warn",
                       WINEDLLOVERRIDES="d3d12,d3d12core,dxgi=n,b", XDG_CACHE_HOME=trial / "cache",
                       XDG_RUNTIME_DIR=trial / "runtime", TMPDIR=trial / "tmp", VKD3D_SHADER_CACHE_PATH=trial)
    if any(not key.startswith("TUXBOX_") for key in (extra_env or {})):
        raise ValueError("extra guest environment must contain only TUXBOX_ controls")
    environment.update(extra_env or {})
    # The provider requests translation through the launcher's per-prefix
    # session. Keep the host worker alive while the sandboxed guest runs.
    with (work / "gpu.log").open("w") as log:
        def report(message):
            log.write(message + "\n")
            log.flush()
        with shader_session(prefix / SOURCE_CACHE.parent, environment, report):
            for key, value in environment.items():
                command.extend(["--setenv", key, str(value)])
            command.extend(["--", "xvfb-run", "-a", "timeout", "-k", "2s", "45s", str(files / "bin/wine"), str(exe)])
            result = subprocess.run(command, stdout=log, stderr=subprocess.STDOUT, timeout=55)
    raw = trial / "result.txt"
    observed = dict(line.split("=", 1) for line in raw.read_text().splitlines()) if raw.exists() else {}
    return exe, result.returncode, observed
