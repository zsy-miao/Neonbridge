"""Opt-in Project Scarlett launch profiles for Linux graphics comparisons."""
from contextlib import contextmanager
from pathlib import Path
import shutil
import subprocess
import sys
from tempfile import TemporaryDirectory


PROFILES = {
    "packaged": "Packaged settings",
    "series-s": "Series S mode (experimental)",
}
CONSOLE_MODE_KEY = r"HKLM\Software\Microsoft\Windows NT\CurrentVersion\OEM"


def choose_profile(env):
    zenity = shutil.which("zenity", path=env.get("PATH"))
    if zenity and (env.get("DISPLAY") or env.get("WAYLAND_DISPLAY")):
        result = subprocess.run(
            [zenity, "--list", "--radiolist", "--title=Project Scarlett graphics",
             "--text=Series S mode uses a temporary copy of the Wine prefix.",
             "--column=Select", "--column=Profile", "--column=ID",
             "--hide-column=3", "--print-column=3",
             "TRUE", PROFILES["packaged"], "packaged", "FALSE", PROFILES["series-s"], "series-s"],
            env=env, capture_output=True, text=True,
        )
        if result.returncode == 1:
            return None
        selected = result.stdout.strip()
        if result.returncode == 0 and selected in PROFILES:
            return selected
        raise ValueError("Could not open the graphics profile menu. Use --graphics-profile instead.")
    if not sys.stdin.isatty():
        raise ValueError("Use --graphics-profile in a non-interactive session")
    print(f"1. {PROFILES['packaged']}\n2. {PROFILES['series-s']} using a temporary prefix")
    try:
        answer = input("Graphics profile [1, 2, or q] ").strip().lower()
    except EOFError:
        return None
    if answer == "q":
        return None
    if answer in ("", "1", "2"):
        return "series-s" if answer == "2" else "packaged"
    raise ValueError("Select graphics profile 1 or 2")


@contextmanager
def graphics_profile(executable, host, selected, *, menu=False):
    if (host.backend not in ("proton", "wine")
            or executable.name.casefold() != "game_scarlett_master.exe"
            or not executable.with_name("shaders.rpf").is_file()):
        raise ValueError("The graphics profiles require Project Scarlett on Linux with Wine or Proton")
    if menu:
        selected = choose_profile(host.env)
    if selected != "series-s":
        yield PROFILES.get(selected)
        return

    prefix = Path(host.env["WINEPREFIX"])
    registry_tool = Path("drive_c/windows/system32/reg.exe")
    if not (prefix / registry_tool).is_file():
        raise ValueError("Series S mode needs an initialized Wine prefix")
    bwrap = shutil.which("bwrap", path=host.env.get("PATH"))
    if not bwrap:
        raise ValueError("Install bubblewrap to run the isolated Series S graphics trial")
    with TemporaryDirectory(dir=prefix.parent, prefix="tuxbox-graphics-") as trial:
        clone = Path(trial) / "prefix"
        subprocess.run(["cp", "-a", "--reflink=auto", "--", str(prefix), str(clone)], check=True)
        # The copied pointer may still target the original prefix's title scratch.
        title_drive = clone / "dosdevices/t:"
        if title_drive.is_symlink():
            title_drive.unlink()
        env, command = host.env, host.command
        host.env = dict(env, WINEPREFIX=str(clone), UMU_RUNTIME_UPDATE="0")
        host.command = [bwrap, "--dev-bind", "/", "/", "--unshare-net", "--", *command]
        try:
            # The title's GDK reads ConsoleMode from this key to select the console model.
            result = host.run(clone / registry_tool,
                              ["add", CONSOLE_MODE_KEY, "/v", "ConsoleMode", "/t", "REG_DWORD", "/d", "96", "/f"],
                              timeout=40, capture=True)
            if result.returncode:
                raise ValueError("Could not select Series S mode in the temporary prefix")
            yield PROFILES[selected]
        finally:
            host.env, host.command = env, command
