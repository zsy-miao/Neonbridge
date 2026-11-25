#!/usr/bin/env python3
"""Check Linux uinput → Proton XInput and the Frosty GameInput adapter on a private X display."""
import argparse
import json
import os
import signal
import shutil
import subprocess
import sys
import time
from pathlib import Path
from tempfile import TemporaryDirectory

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from build import Toolchain
from project_tuxbox.host import Host
from project_tuxbox.inspect import digest


def local_gamepad():
    from evdev import AbsInfo, UInput, ecodes as e

    sticks = [e.ABS_X, e.ABS_Y, e.ABS_RX, e.ABS_RY]
    triggers = [e.ABS_Z, e.ABS_RZ]
    hats = [e.ABS_HAT0X, e.ABS_HAT0Y]
    axes = [(axis, AbsInfo(0, lo, hi, 0, 0, 0)) for axes, lo, hi in
            [(sticks, -32768, 32767), (triggers, 0, 255), (hats, -1, 1)] for axis in axes]
    buttons = [e.BTN_SOUTH, e.BTN_EAST, e.BTN_NORTH, e.BTN_WEST, e.BTN_TL, e.BTN_TR,
               e.BTN_SELECT, e.BTN_START, e.BTN_MODE, e.BTN_THUMBL, e.BTN_THUMBR]
    return UInput({e.EV_KEY: buttons, e.EV_ABS: axes}, name="TuxBox Xbox 360 test controller",
                  vendor=0x045e, product=0x028e, version=0x0114, bustype=e.BUS_USB)



def wait_for(process, report, expected):
    deadline = time.monotonic() + 55
    while time.monotonic() < deadline:
        lines = report.read_text(errors="replace").splitlines() if report.exists() else []
        if expected in lines:
            return
        if process.poll() is not None or any(line.startswith("error=") for line in lines):
            raise RuntimeError(f"Guest exited or failed waiting for {expected}: {lines}")
        time.sleep(0.025)
    raise TimeoutError(expected)


def main():
    parser = argparse.ArgumentParser(description=__doc__, epilog="Disconnect other gamepads for the hot-unplug and keyboard-fallback checks.")
    for option in ("proton", "prefix", "provider", "work", "output"):
        parser.add_argument("--" + option, type=Path, required=True)
    args = parser.parse_args()
    from evdev import ecodes as e
    work = args.work.resolve()
    work.mkdir(parents=True, exist_ok=True)
    tools = Toolchain()
    host = Host("proton", args.proton, args.prefix)
    env = dict(host.env, UMU_RUNTIME_UPDATE="0", WINEDLLOVERRIDES="gameinput=n",
               TUXBOX_KEYBOARD_GAMEPAD="1")
    env.pop("WAYLAND_DISPLAY", None)
    source = ROOT / "tools/reference/controller-input/guest.cpp"
    with TemporaryDirectory(dir=work) as tmp:
        directory = Path(tmp)
        exe = directory / "controller_guest.exe"
        shutil.copy2(args.provider, directory / "GameInput.dll")
        obj = tools.compile(source, directory, "/W4", "/WX")
        tools.link(exe, obj, "user32.lib", "xinput.lib", "kernel32.lib", "/subsystem:windows", "/entry:wmainCRTStartup")
        report = directory / "states.txt"
        script = '''
unshare --user --map-current-user --net "$@" &
child=$!
timeout 28 xdotool search --sync --onlyvisible --name "^TuxBox XInput coexistence$" \\
    windowraise windowfocus mousemove --window %1 50 50 click 1 >/dev/null 2>&1 &
focus=$!
wait "$child"
status=$?
kill "$focus" 2>/dev/null || true
wait "$focus" 2>/dev/null || true
exit "$status"
'''
        command = ["xvfb-run", "-a", "-s", "-screen 0 640x480x24", "bash", "-c", script,
                   "--", *host.command, str(exe), report.name]
        with subprocess.Popen(command, cwd=directory, env=env, start_new_session=True) as process:
            try:
                wait_for(process, report, "connect")
                with local_gamepad() as controller:
                    wait_for(process, report, "hold")
                    for kind, code, value in [(e.EV_KEY, e.BTN_EAST, 1), (e.EV_ABS, e.ABS_X, 24000),
                                               (e.EV_ABS, e.ABS_RZ, 160)]:
                        controller.write(kind, code, value)
                    controller.syn()
                    wait_for(process, report, "release")
                    controller.write(e.EV_KEY, e.BTN_EAST, 0)
                    controller.write(e.EV_ABS, e.ABS_X, 0)
                    controller.write(e.EV_ABS, e.ABS_RZ, 0)
                    controller.syn()
                    wait_for(process, report, "unplug")
                wait_for(process, report, "passed")
                if process.wait(timeout=35):
                    raise RuntimeError("Controller probe failed")
            finally:
                if process.poll() is None:
                    os.killpg(process.pid, signal.SIGTERM)
                    try:
                        process.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        os.killpg(process.pid, signal.SIGKILL)
                        process.wait()
        observed = dict(line.split("=", 1) for line in report.read_text().splitlines() if "=" in line)
        result = dict(host=host.version, provider_sha256=digest(directory / "GameInput.dll"), guest_sha256=digest(exe),
                      fixture="Linux uinput Xbox 360-style USB controller (045e:028e)", host_input="XInputGetState",
                      observed=observed, checks=["connect", "matching GameInput hardware port",
                      "B, left stick X and right trigger", "Z and left-click press/release during held hardware input",
                      "ordered GetNextReading on same physical port", "neutral", "disconnect", "keyboard fallback"])
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2) + "\n")


if __name__ == "__main__":
    main()
