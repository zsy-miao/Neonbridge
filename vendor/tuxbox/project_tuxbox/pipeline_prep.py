"""Warm Frosty's profiled compute pipelines through the title's D3D12 provider."""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
from tempfile import TemporaryDirectory

from .inspect import digest
from .prep_progress import report_progress
from .shader_prep import ROOT


HELPER = ROOT / "tools/pipeline-prep"
COMPUTE_PIPELINES = {("global", 379), ("global", 2227), ("global", 3333), ("global", 2696), ("global", 856),
                     ("material", 37635), ("material", 15458)}
# These pipeline keys occur in the title's v1.0 cache on the TuxBox provider.
CHASE_COMPUTE_PIPELINES = {("global", index) for index in (1825, 2653, 2696, 3385, 3444, 3725)}


def winpath(path):
    return "Z:" + str(path.resolve()).replace("/", "\\")


def device_key(env):
    # vkd3d checks adapter, driver and shader interface compatibility before reuse.
    # Without a device fingerprint, ask vkd3d to check again at the next launch.
    try:
        result = subprocess.run(["vulkaninfo", "--summary"], env=env, capture_output=True,
                                text=True, errors="replace", timeout=6, check=True)
    except (OSError, subprocess.SubprocessError):
        return None
    fields = ("driverVersion", "driverInfo", "deviceUUID", "driverUUID")
    values = [line.strip() for line in result.stdout.splitlines() if line.strip().startswith(fields)]
    return values if any(line.startswith("deviceUUID") for line in values) else None


def archives(executable, env):
    directory = env.get("VKD3D_SHADER_CACHE_PATH")
    if directory:
        # Linux Proton also accepts Unix absolute paths. Other spellings still
        # work in the helper; we simply cannot verify them for an early skip.
        if not Path(directory).is_absolute():
            return []
        base = Path(directory) / f"vkd3d-proton.{executable.name}.cache"
    else:
        base = executable.parent / "vkd3d-proton.cache"
    return [base, base.with_suffix(".cache.write")]


def driver_cache_directories(env):
    # On drivers that store only shader identifiers in vkd3d's archive, clearing
    # the separate driver cache should also trigger preparation on the next boot.
    base = Path(env.get("XDG_CACHE_HOME") or Path.home() / ".cache")
    nvidia = Path(env.get("__GL_SHADER_DISK_CACHE_PATH") or base / "nvidia")
    mesa = Path(env.get("MESA_SHADER_CACHE_DIR") or base)
    result = {}
    for path in (nvidia / "GLCache", mesa / "mesa_shader_cache", mesa / "mesa_shader_cache_db"):
        try:
            stat = path.stat()
        except FileNotFoundError:
            continue
        result[str(path)] = [stat.st_dev, stat.st_ino]
    return result


def archive_files(paths):
    entries = []
    for path in paths:
        try:
            with path.open("rb") as stream:
                stat = os.fstat(stream.fileno())
                header = stream.read(48)
        except FileNotFoundError:
            continue
        if len(header) == 48 and header.startswith(b"VKS") and stat.st_size > 48:
            entries.append([stat.st_dev, stat.st_ino, header.hex(), stat.st_size])
    return entries


def still_cached(current, recorded):
    return any(now[:3] == old[:3] and now[3] >= old[3]
               for now in current for old in recorded
               if isinstance(old, list) and len(old) == 4 and isinstance(old[3], int))


def helper_binary(work, filename, source_id):
    from build import RUST_STD_LIBS, Toolchain

    # vkd3d keys custom cache directories by the Windows executable basename.
    # The separate helper directory keeps the retail image untouched.
    output = work / source_id / filename
    if not output.is_file():
        output.parent.mkdir(exist_ok=True)
        with TemporaryDirectory(dir=work) as temp:
            stage = Path(temp)
            tools = Toolchain()
            worker = tools.rust_library("tuxbox-pipeline-prep", target_dir=work / "cargo")
            tools.link(stage / filename, worker, "/entry:wmainCRTStartup", "d3d12.lib", *RUST_STD_LIBS)
            os.replace(stage / filename, output)
    return output


def prepare_pipelines(executable, host, plan, cache, progress=None, *, standalone=False):
    import fcntl

    if host.env.get("VKD3D_SHADER_CACHE_PATH") == "0":
        return
    indices = CHASE_COMPUTE_PIPELINES if standalone else COMPUTE_PIPELINES
    records = [r for r in plan["shaders"] if (r["archive"], r["entry"]) in indices]
    if len(records) != len(indices):
        raise ValueError("Missing Frosty chase pipeline profile")
    key = "dxbc_sha256" if standalone else "cache_key"
    shaders = {r["entry"]: cache / (r[key] + ".cso") for r in records}
    work = cache.parent / "tuxbox-pipelines"
    work.mkdir(parents=True, exist_ok=True)
    with (work / "prepare.lock").open("a+b") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        state = work / "pipelines.json"
        try:
            saved = json.loads(state.read_text())
        except (OSError, ValueError, UnicodeError):
            saved = {}
        env = host.env.copy()
        if standalone or env.get("TUXBOX_REFERENCE_CBV") == "1":
            from .console_cbv import configure_console_cbv
            configure_console_cbv(host.backend, env, executable, lambda message: print(message, file=sys.stderr))
        gpu = device_key(env)
        inputs = [ROOT / "Cargo.toml", ROOT / "Cargo.lock", ROOT / ".cargo/config.toml",
                  HELPER / "Cargo.toml", *sorted((HELPER / "src").rglob("*.rs"))]
        source = hashlib.sha256("".join(digest(path) for path in inputs).encode("ascii")).hexdigest()
        paths = archives(executable, env)
        settings = {k: v for k, v in env.items() if k.startswith(("VK_", "VKD3D_", "DXVK_", "__GL_", "MESA_", "DRI_"))
                    or k in ("XDG_CACHE_HOME", "PROTONPATH", "STEAM_COMPAT_SHADER_PATH")}
        provider = digest(executable.with_name(plan["provider"])) if standalone else plan["provider_sha256"]
        identity = dict(driver=gpu, proton=host.version, provider=provider,
                        executable=plan["executable_sha256"], helper=source, settings=settings,
                        dxil=[digest(path) for path in shaders.values()])
        if (gpu and isinstance(saved, dict) and identity == saved.get("identity")
                and driver_cache_directories(env) == saved.get("driver_cache_dirs")
                and isinstance(saved.get("archives"), list)
                and still_cached(archive_files(paths), saved["archives"])):
            report_progress(progress, f"Frosty compute pipelines ready: {len(records)} cached.",
                            len(records), len(records))
            return
        message = f"Preparing {len(records)} Frosty compute pipelines on this GPU…"
        report_progress(progress, message, 0, len(records))
        helper = helper_binary(work, executable.name, source)
        with TemporaryDirectory(dir=work) as temp:
            stage = Path(temp)
            report = stage / "compiled.txt"
            args = [winpath(executable.parent), winpath(report)]
            if standalone:
                args.append("--tuxbox")
            args.extend(winpath(path) for path in shaders.values())
            # Keep the actual title's Proton log for the actual title.
            env.pop("PROTON_LOG", None)
            env["WINEDEBUG"] = "-all"
            process = subprocess.run([*host.command, str(helper), *args], cwd=executable.parent, env=env)
            if process.returncode:
                detail = report.read_text(errors="replace").strip() if report.exists() else "no GPU result"
                raise ValueError(f"Pipeline preparation exited with {process.returncode}: {detail}")
            by_key = {key: float(ms) for key, ms in (row.split() for row in report.read_text().splitlines())}
            if set(by_key) != {path.stem for path in shaders.values()}:
                raise ValueError("GPU preparation did not finish all profiled Frosty pipelines")
            timings = {entry: by_key[path.stem] for entry, path in shaders.items()}
            files = archive_files(paths)
            # The write archive may be renamed to the main archive at next startup.
            # Prefer its identity when this run appended new pipelines there.
            result = dict(identity=identity, archives=files[-1:], pipeline_ms=timings,
                          driver_cache_dirs=driver_cache_directories(env))
            pending = stage / "finished.json"
            pending.write_text(json.dumps(result, indent=2) + "\n")
            os.replace(pending, state)
        report_progress(progress, f"Frosty compute pipelines ready: {len(records)} created.",
                        len(records), len(records))
