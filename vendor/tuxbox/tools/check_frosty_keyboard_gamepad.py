#!/usr/bin/env python3
"""Verify the Frosty desktop-input bridge through Linux X11, Proton, and GameInput."""
import argparse
import json
from pathlib import Path
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from build import Toolchain
from project_tuxbox.host import Host
from project_tuxbox.inspect import digest


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--provider", type=Path, help="GameInput.dll to test (default: build/gameinput/GameInput.dll)")
    for option in ("proton", "prefix", "work", "output"):
        parser.add_argument("--" + option, required=True, type=Path)
    args = parser.parse_args()
    work = args.work.resolve()
    work.mkdir(exist_ok=True, parents=True)
    tools = Toolchain()
    provider = work / "GameInput.dll"
    source = (args.provider or ROOT / "build/gameinput/GameInput.dll").resolve()
    if source != provider:
        shutil.copy2(source, provider)
    imports = ROOT / "runtime/gameinput/guest-imports.def"
    subprocess.run([tools.linker, "/lib", "/nologo", "/machine:x64",
                    f"/def:{imports}", "/out:GameInput.lib"], cwd=work, check=True)
    exe = work / "keyboard_guest.exe"
    obj = tools.compile(ROOT / "tools/reference/menu-input/guest.cpp", work, "/W4", "/WX")
    tools.link(exe, obj, "GameInput.lib", "user32.lib", "xinput.lib", "kernel32.lib", "/subsystem:windows", "/entry:wmainCRTStartup")
    host = Host("proton", args.proton, args.prefix)
    env = dict(host.env, UMU_RUNTIME_UPDATE="0", WINEDLLOVERRIDES="gameinput=n,b")
    env.pop("WAYLAND_DISPLAY", None)
    result = dict(host=host.version, provider_sha256=digest(provider), guest_sha256=digest(exe),
                  runs={})
    for mode in ("on", "off"):
        if mode == "on":
            env["TUXBOX_KEYBOARD_GAMEPAD"] = "1"
        else:
            env.pop("TUXBOX_KEYBOARD_GAMEPAD", None)
        # Both the focus action and the guest run exclusively on the private X display.
        script = '''
set -eu
unshare --user --map-current-user --net "$@" &
child=$!
timeout 15 xdotool search --sync --onlyvisible --name "^TuxBox keyboard controller test$" \\
    windowraise windowfocus mousemove --window %1 60 60 click 1 || true
while kill -0 "$child" 2>/dev/null; do
    if [[ -f input.cmd ]]; then
        read -r -a event < input.cmd
        xdotool "${event[@]}"
        mv input.cmd input.done
    else
        sleep 0.005
    fi
done
wait "$child"
'''
        report = work / (mode + ".txt")
        report.unlink(missing_ok=True)
        for name in ("input.cmd", "input.done", "input.next"):
            (work / name).unlink(missing_ok=True)
        subprocess.run(["xvfb-run", "-a", "-s", "-screen 0 640x480x24", "bash", "-c", script, "--",
                        *host.command, str(exe), mode, report.name], cwd=work, env=env, check=True, timeout=40)
        result["runs"][mode] = dict(line.split("=", 1) for line in report.read_text().splitlines())
    if result["runs"]["on"].get("ordered_edges") != "48" or result["runs"]["off"].get("keyboard_pad_absent") != "1":
        raise ValueError("GameInput control did not verify both modes")
    if any(result["runs"]["on"].get(name) != "1" for name in
           ("mouse_confirm", "mouse_aim_fire", "mouse_idle_neutral", "cursor_restored")) or (
            result["runs"]["on"].get("mouse_look_signed_axes") != "2"):
        raise ValueError("Mouse control did not verify buttons, motion, and cursor release")
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2) + "\n")


if __name__ == "__main__":
    main()
