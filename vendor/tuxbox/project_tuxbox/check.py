"""Run every check a change needs before it can be merged.

The first group is what CI runs. The second builds every provider and guest and
runs Lanes and the other guest fixtures on the GPU, which CI cannot do, so each
contributor runs it and reports the result.
"""
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
from time import monotonic

ROOT = Path(__file__).resolve().parents[1]
def host_target():
    info = subprocess.check_output(["rustc", "-vV"], text=True)
    return next(line.removeprefix("host: ") for line in info.splitlines() if line.startswith("host: "))


def code_steps(python, target):
    cargo = ["cargo", "test", "--locked", "--target", target]
    return [
        # The Python tests that run translated shaders need this binary.
        ("Build the shader compiler", ["cargo", "build", "--release", "--locked", "--target", target,
                                       "--manifest-path", "tools/shader-compiler/Cargo.toml",
                                       "--target-dir", "build/shader-compiler"]),
        ("Python tests", [python, "-m", "unittest", "discover", "-s", "tests", "-q"]),
        ("Shader compiler tests", [*cargo, "--manifest-path", "tools/shader-compiler/Cargo.toml"]),
        ("DXBC tests", [*cargo, "-p", "tuxbox-dxbc", "--features", "native-bindings"]),
        ("XG layout tests", [*cargo, "-p", "tuxbox-xg-layout"]),
        ("D3D12 Windows typecheck", ["cargo", "check", "--locked", "--target", "x86_64-pc-windows-msvc",
                                     "-p", "tuxbox-d3d12", "-p", "tuxbox-bvh"]),
    ]


def gpu_steps(python, output, host_options):
    # No `--only`: every suite, Lanes included.
    return [("Build", [python, "build.py"]),
            ("Lanes and fixtures on the GPU",
             [python, "tuxbox.py", "verify", "--output", str(output), *host_options])]


def native_test_environment(target, dxc):
    """What the Python tests need to translate and run shaders, when present."""
    env = os.environ.copy()
    xshader = ROOT / "build/shader-compiler" / target / "release" / ("xshader.exe" if os.name == "nt" else "xshader")
    if xshader.is_file():
        env.setdefault("TUXBOX_XSHADER", str(xshader))
    if dxc:
        env.setdefault("DXC", str(dxc))
    return env


def find_native_dxc():
    from build import find_dxc

    try:
        return find_dxc(None)
    except ValueError:
        return None


def isolated_inputs(results, options, windows_dxc):
    """What the isolated Linux GPU checks need, or why they cannot run.

    They need a Proton installation and prefix to copy, and the Microsoft DXC
    libraries for Windows. The adapter is the one asked for, or the one the
    guest checks ran on.
    """
    given = {name: options[options.index(name) + 1] for name in ("--proton", "--prefix", "--adapter")
             if name in options}
    if "--proton" not in given or "--prefix" not in given or windows_dxc is None:
        return None, "pass --proton, --prefix and --windows-dxc"
    adapter = given.get("--adapter")
    if adapter is None and results.is_file():
        found = re.search(r"pci=([0-9a-fA-F]{4}:[0-9a-fA-F]{4})", results.read_text(encoding="utf-8"))
        adapter = found and found.group(1)
    if adapter is None:
        return None, "no adapter known; pass --adapter"
    return {"proton": given["--proton"], "prefix": given["--prefix"], "compiler-dir": str(windows_dxc),
            "adapter": adapter}, None


def regression_step(python, results, options, windows_dxc):
    """The command for the isolated D3D12 controls, or why they cannot run."""
    inputs, reason = isolated_inputs(results, options, windows_dxc)
    if not inputs:
        return None, reason
    work, output = results.parent / "regression-work", results.parent / "regressions.json"
    # The runner refuses to write over an earlier run.
    shutil.rmtree(work, ignore_errors=True)
    output.unlink(missing_ok=True)
    from tools.check_d3d12_regressions import CASES

    command = [python, "tools/check_d3d12_regressions.py",
               "--build-dir", "build/d3d12-smoke", "--xmem", "build/xmem-smoke/xmem.dll",
               "--work", str(work), "--output", str(output)]
    for name, value in inputs.items():
        command += [f"--{name}", value]
    for case in CASES:
        command += ["--case", case]
    return command, None


def translator_step(python, results, options, windows_dxc, dxc):
    """The command for the translator's GPU checks, or why they cannot run."""
    inputs, reason = isolated_inputs(results, options, windows_dxc)
    if not inputs:
        return None, reason
    if not dxc:
        return None, "no native DXC found"
    if not shutil.which("llvm-mc"):
        return None, "install llvm-mc from LLVM"
    work = results.parent / "translator-work"
    shutil.rmtree(work, ignore_errors=True)
    command = [python, "tools/check_translator_gpu.py", "--provider", "build/d3d12-smoke/d3d12_xs.dll",
               "--dxc", str(dxc), "--work", str(work)]
    for name, value in inputs.items():
        command += [f"--{name}", value]
    return command, None


def benchmark_readings(path):
    """The `name: value` timings of the Lanes benchmark recorded in a results file."""
    data = json.loads(Path(path).read_text(encoding="utf-8"))
    for row in data.get("checks", ()):
        if row.get("gate") == "lanes-benchmark" and row.get("passed"):
            return {name: float(value) for name, value in re.findall(r"(\w+_us)=([0-9.]+)", row["receipt"])}
    return {}


def compare_benchmarks(baseline, current):
    """Lines showing each timing against the baseline. Lower is better."""
    old, new = benchmark_readings(baseline), benchmark_readings(current)
    shared = [name for name in new if old.get(name)]
    if not shared:
        return ["No benchmark readings to compare."]
    return [f"  {name:20s} {old[name]:9.1f} -> {new[name]:9.1f} us  {new[name] / old[name] - 1:+.0%}"
            for name in shared]


def run_checks(*, gpu=True, host_options=(), baseline=None, windows_dxc=None, run=subprocess.run,
               report=print):
    """Run the steps in order and return a process exit code.

    Every step runs even after a failure, except that the GPU check is skipped
    when a build it depends on failed.
    """
    output = ROOT / "build/check/results.json"
    output.parent.mkdir(parents=True, exist_ok=True)
    target = host_target()
    steps = code_steps(sys.executable, target)
    if gpu:
        steps += gpu_steps(sys.executable, output, list(host_options))
    results = []
    build_failed = False
    for name, command in steps:
        if name.endswith("on the GPU") and build_failed:
            results.append((name, "skipped: a build failed", 0.0))
            continue
        report(f"== {name}")
        started = monotonic()
        env = (native_test_environment(target, find_native_dxc()) if name == "Python tests"
               else os.environ.copy())
        passed = run(command, cwd=ROOT, env=env).returncode == 0
        build_failed |= name == "Build" and not passed
        results.append((name, "passed" if passed else "FAILED", monotonic() - started))
    skipped = []
    if gpu:
        for name in ("D3D12 regression controls", "Shader translator on the GPU"):
            if build_failed:
                command, reason = None, "a build failed"
            elif name.startswith("D3D12"):
                command, reason = regression_step(sys.executable, output, list(host_options), windows_dxc)
            else:
                command, reason = translator_step(sys.executable, output, list(host_options), windows_dxc,
                                                  find_native_dxc())
            if command:
                report(f"== {name}")
                started = monotonic()
                passed = run(command, cwd=ROOT, env=os.environ.copy()).returncode == 0
                results.append((name, "passed" if passed else "FAILED", monotonic() - started))
            else:
                results.append((name, f"not run: {reason}", 0.0))
                skipped.append(f"{name}: {reason}")
    report("\nSummary")
    for name, result, seconds in results:
        report(f"  {result:8s} {name} ({seconds:.0f}s)")
    failed = any(result != "passed" and not result.startswith("not run") for _, result, _ in results)
    if gpu and baseline and output.is_file():
        report(f"\nBenchmark against {baseline}")
        for line in compare_benchmarks(baseline, output):
            report(line)
    if not gpu:
        report("\nThe GPU checks were not run. A change cannot be merged until Lanes passes on a GPU.")
    elif not failed:
        report(f"\nAll checks that ran passed. Attach the GPU results to the pull request: {output}"
               if skipped else f"\nAll checks passed. Attach the GPU results to the pull request: {output}")
        if skipped:
            for line in skipped:
                report(f"Did not run: {line}. On Linux a change needs these too.")
    return 1 if failed else 0
