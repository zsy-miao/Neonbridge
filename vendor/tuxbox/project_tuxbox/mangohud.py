"""Record a Linux game's Vulkan presents alongside its TuxBox log."""
from contextlib import contextmanager
from pathlib import Path
import re
import shutil
import subprocess
from tempfile import mkdtemp
from time import monotonic

from .performance import summarize_capture


def telemetry_gpu(env):
    address = env.get("TUXBOX_MANGOHUD_PCI")
    if address:
        if not re.fullmatch(r"[0-9a-fA-F]{4}:[0-9a-fA-F]{2}:[0-9a-fA-F]{2}\.[0-7]", address):
            raise ValueError("TUXBOX_MANGOHUD_PCI requires a PCI address such as 0000:01:00.0")
        return address.lower()
    # MangoHud can enumerate DRM nodes even for GPUs unavailable to Vulkan.
    # On dual-GPU systems it otherwise logs its last node's hardware counters.
    nodes = list(Path("/sys/class/drm").glob("renderD*/device"))
    if len(nodes) > 1:
        try:
            probe = subprocess.run(["vulkaninfo", "--summary"], env={**env, "DISABLE_MANGOHUD": "1"}, capture_output=True,
                                   text=True, errors="replace", timeout=4, check=True)
            vendors = re.findall(r"(?m)^\s*vendorID\s*=\s*(0x[0-9a-fA-F]+)", probe.stdout)
            devices = re.findall(r"(?m)^\s*deviceID\s*=\s*(0x[0-9a-fA-F]+)", probe.stdout)
            if len(vendors) == len(devices) == 1:
                matches = [node.resolve().name for node in nodes
                           if int((node / "vendor").read_text(), 16) == int(vendors[0], 16)
                           and int((node / "device").read_text(), 16) == int(devices[0], 16)]
                if len(matches) == 1:
                    return matches[0]
        except (OSError, ValueError, subprocess.SubprocessError):
            pass
    return None


@contextmanager
def performance_capture(backend, env, executable, log):
    if backend not in ("wine", "proton"):
        raise ValueError("--mangohud requires a Linux Wine or Proton backend")
    if not shutil.which("mangohud", path=env.get("PATH")):
        raise ValueError("Install MangoHud and its Vulkan layer to use --mangohud")

    gpu = telemetry_gpu(env)
    directory = Path(mkdtemp(prefix="mangohud-", dir=log.path.parent)).resolve()
    config = directory / "MangoHud.conf"
    config.write_text("fps\nframetime\nframe_timing\ncpu_stats\ngpu_stats\nvram\nram\n"
                      "autostart_log=1\nlog_interval=0\npermit_upload=0\n"
                      f"output_folder={directory}\n" + (f"pci_dev={gpu}\n" if gpu else ""), encoding="utf-8")
    # MangoHud ignores its config file when an inline config is present.
    env.pop("MANGOHUD_CONFIG", None)
    env.pop("DISABLE_MANGOHUD", None)
    env.update(MANGOHUD="1", MANGOHUD_CONFIGFILE=str(config))
    log.status(f"MangoHud frame capture: {directory}")
    started = monotonic()
    try:
        yield
    finally:
        try:
            summary = summarize_capture(directory, executable, run_seconds=monotonic() - started)
        except (OSError, ValueError) as exc:
            log.status(f"Could not summarize MangoHud frames: {exc}")
        else:
            if summary:
                log.status(f"MangoHud performance summary: {summary}")
            else:
                log.status(f"MangoHud recorded no frames for {executable.name}. Check {directory}")
