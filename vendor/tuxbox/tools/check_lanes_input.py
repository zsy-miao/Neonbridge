#!/usr/bin/env python3
"""Play Lanes through the launcher with XTest key presses and check the displayed board.

On a Wayland desktop the game runs on a private rootful Xwayland, so neither the desktop nor its
focused window receives the input. Elsewhere it uses the current X display; run it under xvfb-run to
keep it off an X11 desktop. After each key press the board is sampled from the X root window, where
the presented frames land. Every check reads the board itself, so it holds for any course seed:
Left and Right move the highlighted lane, steering into the next row's barrier ends play with the
COLLISION banner and ignores later moves, R restarts in the middle lane, and Escape closes the game
with exit status 0.
"""
import argparse
from contextlib import nullcontext
import ctypes
import ctypes.util
from dataclasses import dataclass
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from project_tuxbox.inspect import digest
from project_tuxbox.verify import private_x_server, screen_capture_problem

# Board layout and colors from examples/lanes/screen.cpp, in client coordinates.
WIDTH, HEIGHT = 520, 640
LEFT, TOP, BOTTOM = WIDTH // 18, 104, HEIGHT - 88
RIGHT = WIDTH - LEFT
MID = (TOP + BOTTOM) // 2
NEXT_ROW = BOTTOM - 42 - (BOTTOM - TOP - 42) // 5 // 2  # centre of the row the next move lands on
INK, PAPER, TEAL, RED = (21, 26, 38), (242, 245, 249), (95, 235, 202), (250, 105, 88)
ACTIVE, IDLE = (42, 64, 76), (34, 42, 59)
# The finish banner's title is 3x-scale lettering whose first and last columns are lit.
BANNERS = {(len("COLLISION") * 6 - 1) * 3: "crashed", (len("COURSE COMPLETE") * 6 - 1) * 3: "won"}
KEYS = {-1: "Left", 0: "space", 1: "Right"}
PACKAGE = ("lanes.exe", "d3d12_xs.dll", "GameInput.dll", "xmem.dll")
# GameInput reads the keyboard only while the game is the Win32 foreground window. A first key can
# be lost while Wine is still activating the new window (seen once in 13 runs, right after a full
# verify). The board then stays exactly at the start, so it is safe to focus again and repeat that
# one press; any other board is a failure.
FIRST_PRESS_ATTEMPTS, FIRST_PRESS_WAIT, FOCUS_SETTLE = 3, 1.5, 0.5
PRESS_WAIT, IGNORED_WAIT = 5.0, 0.75  # how long a move may take to show; how long an ignored one is watched


class CheckFailed(Exception):
    pass


def center(lane):
    return LEFT + (2 * lane + 1) * (RIGHT - LEFT) // 6


@dataclass(frozen=True)
class Board:
    lane: int
    finish: str  # "playing", "crashed" or "won"
    hazard: int | None  # barrier lane in the next row; None once the course is used up


def read_board(pixel):
    """Decode the Lanes board from pixel(x, y) -> (r, g, b), or raise ValueError."""
    bands = [pixel(center(lane), TOP + 3) for lane in range(3)]
    if bands.count(ACTIVE) != 1 or bands.count(IDLE) != 2:
        raise ValueError(f"no Lanes board: lane colors {bands}")
    lane = bands.index(ACTIVE)
    if pixel(center(lane), BOTTOM - 32) != TEAL:
        raise ValueError(f"no player in the highlighted lane {lane}")
    finish = "playing"
    if pixel(LEFT + 2, MID) == INK:  # the banner covers the lanes
        lit = [x for y in range(MID - 31, MID - 10) for x in range(LEFT, RIGHT) if pixel(x, y) == PAPER]
        width = max(lit) - min(lit) + 1 if lit else 0
        if width not in BANNERS:
            raise ValueError(f"unrecognized finish banner, title {width} px wide")
        finish = BANNERS[width]
    hazards = [h for h in range(3) if pixel(center(h), NEXT_ROW - 10) == RED]
    if len(hazards) > 1:
        raise ValueError(f"next row shows barriers in lanes {hazards}")
    return Board(lane, finish, hazards[0] if hazards else None)


def step_toward(lane, hazard):
    """Move that lands on the barrier, or the safe step that brings it within reach."""
    return max(-1, min(1, hazard - lane))


class XImage(ctypes.Structure):
    _fields_ = [("width", ctypes.c_int), ("height", ctypes.c_int), ("xoffset", ctypes.c_int),
                ("format", ctypes.c_int), ("data", ctypes.c_void_p), ("byte_order", ctypes.c_int),
                ("bitmap_unit", ctypes.c_int), ("bitmap_bit_order", ctypes.c_int),
                ("bitmap_pad", ctypes.c_int), ("depth", ctypes.c_int), ("bytes_per_line", ctypes.c_int),
                ("bits_per_pixel", ctypes.c_int), ("red_mask", ctypes.c_ulong),
                ("green_mask", ctypes.c_ulong), ("blue_mask", ctypes.c_ulong)]


class XWindowAttributes(ctypes.Structure):
    _fields_ = [("x", ctypes.c_int), ("y", ctypes.c_int), ("width", ctypes.c_int), ("height", ctypes.c_int),
                ("border_width", ctypes.c_int), ("depth", ctypes.c_int), ("visual", ctypes.c_void_p),
                ("root", ctypes.c_ulong), ("class_", ctypes.c_int), ("bit_gravity", ctypes.c_int),
                ("win_gravity", ctypes.c_int), ("backing_store", ctypes.c_int),
                ("backing_planes", ctypes.c_ulong), ("backing_pixel", ctypes.c_ulong),
                ("save_under", ctypes.c_int), ("colormap", ctypes.c_ulong), ("map_installed", ctypes.c_int),
                ("map_state", ctypes.c_int), ("all_event_masks", ctypes.c_long),
                ("your_event_mask", ctypes.c_long), ("do_not_propagate_mask", ctypes.c_long),
                ("override_redirect", ctypes.c_int), ("screen", ctypes.c_void_p)]


def frame_pixels(data, width, height, stride, masks, msb_first=False):
    """pixel(x, y) -> (r, g, b) over a 32-bit ZPixmap copied from the server."""
    shifts = [(mask & -mask).bit_length() - 1 for mask in masks]
    order = "big" if msb_first else "little"

    def pixel(x, y):
        if not (0 <= x < width and 0 <= y < height):
            raise ValueError(f"sample ({x}, {y}) is outside the {width}x{height} capture")
        value = int.from_bytes(data[y * stride + 4 * x:y * stride + 4 * x + 4], order)
        return tuple((value & mask) >> shift for mask, shift in zip(masks, shifts))
    return pixel


class Display:
    """The Xlib and XTest calls the check needs, on one display."""

    def __init__(self, name, xauthority):
        c = ctypes
        self.x11 = x11 = c.CDLL(c.util.find_library("X11") or "libX11.so.6")
        self.xtst = xtst = c.CDLL(c.util.find_library("Xtst") or "libXtst.so.6")
        p, w = c.c_void_p, c.c_ulong
        for fn, res, args in (
                ("XOpenDisplay", p, [c.c_char_p]), ("XDefaultRootWindow", w, [p]),
                ("XQueryTree", c.c_int, [p, w, c.POINTER(w), c.POINTER(w), c.POINTER(c.POINTER(w)), c.POINTER(c.c_uint)]),
                ("XFetchName", c.c_int, [p, w, c.POINTER(c.c_char_p)]), ("XFree", c.c_int, [p]),
                ("XGetWindowAttributes", c.c_int, [p, w, c.POINTER(XWindowAttributes)]),
                ("XTranslateCoordinates", c.c_int, [p, w, w, c.c_int, c.c_int, c.POINTER(c.c_int),
                                                    c.POINTER(c.c_int), c.POINTER(w)]),
                ("XRaiseWindow", c.c_int, [p, w]), ("XSetInputFocus", c.c_int, [p, w, c.c_int, w]),
                ("XGetInputFocus", c.c_int, [p, c.POINTER(w), c.POINTER(c.c_int)]),
                ("XGetImage", c.POINTER(XImage), [p, w, c.c_int, c.c_int, c.c_uint, c.c_uint, w, c.c_int]),
                ("XDestroyImage", c.c_int, [c.POINTER(XImage)]), ("XStringToKeysym", w, [c.c_char_p]),
                ("XKeysymToKeycode", c.c_ubyte, [p, w]), ("XSync", c.c_int, [p, c.c_int]),
                ("XCloseDisplay", c.c_int, [p])):
            getattr(x11, fn).restype, getattr(x11, fn).argtypes = res, args
        xtst.XTestFakeKeyEvent.restype = c.c_int
        xtst.XTestFakeKeyEvent.argtypes = [p, c.c_uint, c.c_int, w]
        handler_type = c.CFUNCTYPE(c.c_int, p, p)
        x11.XSetErrorHandler.restype = p
        x11.XSetErrorHandler.argtypes = [handler_type]
        self.errors = 0

        def record(_display, _event):
            self.errors += 1  # Xlib's default handler would exit the process
            return 0
        self._handler = handler_type(record)
        x11.XSetErrorHandler(self._handler)
        saved = os.environ.get("XAUTHORITY")
        if xauthority:
            os.environ["XAUTHORITY"] = xauthority
        try:
            self.dpy = x11.XOpenDisplay(name.encode())
        finally:
            if xauthority:
                if saved is None:
                    os.environ.pop("XAUTHORITY", None)
                else:
                    os.environ["XAUTHORITY"] = saved
        if not self.dpy:
            raise CheckFailed(f"cannot open X display {name}")
        self.root = x11.XDefaultRootWindow(self.dpy)

    def close(self):
        self.x11.XCloseDisplay(self.dpy)

    def find_window(self, title):
        """(window, x, y) of the viewable WIDTHxHEIGHT window with this title, or None."""
        c, x11 = ctypes, self.x11
        pending = [self.root]
        while pending:
            parent = pending.pop()
            root, up, children, count = c.c_ulong(), c.c_ulong(), c.POINTER(c.c_ulong)(), c.c_uint()
            if not x11.XQueryTree(self.dpy, parent, c.byref(root), c.byref(up), c.byref(children), c.byref(count)):
                continue
            windows = [children[i] for i in range(count.value)]
            if children:
                x11.XFree(children)
            for window in windows:
                pending.append(window)
                name = c.c_char_p()
                if not x11.XFetchName(self.dpy, window, c.byref(name)) or not name.value:
                    continue
                matched = name.value == title.encode()
                x11.XFree(ctypes.cast(name, c.c_void_p))
                attributes = XWindowAttributes()
                if not (matched and x11.XGetWindowAttributes(self.dpy, window, c.byref(attributes))):
                    continue
                if attributes.map_state == 2 and (attributes.width, attributes.height) == (WIDTH, HEIGHT):
                    x, y, child = c.c_int(), c.c_int(), c.c_ulong()
                    x11.XTranslateCoordinates(self.dpy, window, self.root, 0, 0, c.byref(x), c.byref(y), c.byref(child))
                    return window, x.value, y.value
        return None

    def focus(self, window):
        """Give the window the X input focus; return whether the server reports it there."""
        self.x11.XRaiseWindow(self.dpy, window)
        self.x11.XSetInputFocus(self.dpy, window, 2, 0)  # RevertToParent, CurrentTime
        self.x11.XSync(self.dpy, 0)
        deadline = time.monotonic() + 2
        while True:
            current, revert = ctypes.c_ulong(), ctypes.c_int()
            self.x11.XGetInputFocus(self.dpy, ctypes.byref(current), ctypes.byref(revert))
            if current.value == window or time.monotonic() > deadline:
                return current.value == window
            time.sleep(0.05)

    def capture(self, x, y):
        """pixel(x, y) over the board's on-screen rectangle, read from the root window."""
        image = self.x11.XGetImage(self.dpy, self.root, x, y, WIDTH, HEIGHT, 0xFFFFFFFF, 2)  # ZPixmap
        if not image:
            raise ValueError("XGetImage on the root window failed")
        try:
            img = image.contents
            if img.bits_per_pixel != 32:
                raise ValueError(f"{img.bits_per_pixel}-bit root window; expected 32")
            data = ctypes.string_at(img.data, img.bytes_per_line * img.height)
            masks = (img.red_mask, img.green_mask, img.blue_mask)
            return frame_pixels(data, img.width, img.height, img.bytes_per_line, masks, img.byte_order == 1)
        finally:
            self.x11.XDestroyImage(image)

    def press(self, key, hold=0.12):
        # Held across several game polls: GameInput samples GetAsyncKeyState once per frame.
        code = self.x11.XKeysymToKeycode(self.dpy, self.x11.XStringToKeysym(key.encode()))
        if not code:
            raise CheckFailed(f"no keycode for {key}")
        self.xtst.XTestFakeKeyEvent(self.dpy, code, 1, 0)
        self.x11.XSync(self.dpy, 0)
        time.sleep(hold)
        self.xtst.XTestFakeKeyEvent(self.dpy, code, 0, 0)
        self.x11.XSync(self.dpy, 0)


def prefix_processes(prefix):
    """PIDs other than this one whose Wine or Proton prefix variable points into the prefix.

    Wine processes inside Proton's container see WINEPREFIX=<prefix>/pfx, so match the directory
    and anything below it rather than the exact value.
    """
    needle = os.fsencode(str(prefix))
    keys = (b"WINEPREFIX", b"STEAM_COMPAT_DATA_PATH")
    found = []
    for entry in Path("/proc").iterdir():
        if entry.name.isdigit() and int(entry.name) != os.getpid():
            try:
                variables = (entry / "environ").read_bytes().split(b"\0")
            except OSError:
                continue
            values = (value for key, _, value in (v.partition(b"=") for v in variables) if key in keys)
            if any(value == needle or value.startswith(needle + b"/") for value in values):
                found.append(int(entry.name))
    return found


def play(display, window, origin, steps, game):
    """Drive one course from the start position, append each step to steps, return accepted moves."""
    last = {}

    def wait(predicate, what, timeout=None):
        deadline = time.monotonic() + (PRESS_WAIT if timeout is None else timeout)
        while time.monotonic() < deadline:
            if game.poll() is not None:
                raise CheckFailed(f"{what}: the game exited with {game.returncode}")
            try:
                last["board"] = board = read_board(display.capture(*origin))
                if predicate(board):
                    return board
            except ValueError as error:
                last["board"] = str(error)
            time.sleep(0.05)
        raise CheckFailed(f"{what}: last board {last.get('board')}")

    def press(key, predicate, what, attempts=1):
        before = last.get("board")
        attempt = 1
        while True:
            display.press(key)
            try:
                board = wait(predicate, what, timeout=FIRST_PRESS_WAIT if attempt < attempts else None)
                break
            except CheckFailed:
                # Retry only a press that never arrived: any other board is a real failure.
                if attempt >= attempts or last.get("board") != before:
                    raise
                attempt += 1
                display.focus(window)
                time.sleep(FOCUS_SETTLE)
        steps.append(dict(key=key, before=vars(before) if isinstance(before, Board) else before,
                          after=vars(board), **({"attempts": attempt} if attempt > 1 else {})))
        return board

    board = wait(lambda b: b.lane == 1 and b.finish == "playing" and b.hazard is not None,
                 "start position", timeout=10.0)
    steps.append(dict(key=None, after=vars(board)))
    time.sleep(FOCUS_SETTLE)  # let Wine make the game the foreground window before the first key
    # A safe sideways move: the highlighted lane must follow the key.
    target = 0 if board.hazard != 0 else 2
    board = press(KEYS[target - board.lane], lambda b: b.lane == target and b.finish == "playing", "safe move",
                  attempts=FIRST_PRESS_ATTEMPTS)
    moves = 1
    # Steer into the next barrier; when it is two lanes away, the middle lane is a safe step.
    while board.finish == "playing":
        if board.hazard is None:
            raise CheckFailed("the course ended before a collision")
        step, lane = step_toward(board.lane, board.hazard), board.lane
        crash = abs(board.hazard - lane) <= 1
        board = press(KEYS[step], (lambda b: b.finish == "crashed" and b.lane == lane + step) if crash
                      else (lambda b: b.finish == "playing" and b.lane == lane + step),
                      "collision" if crash else "step toward the barrier")
        moves += 1
    # A crashed game ignores moves: the board must not change.
    crashed = board
    ignored = KEYS[1 if crashed.lane < 2 else -1]
    display.press(ignored)
    time.sleep(IGNORED_WAIT)
    after = read_board(display.capture(*origin))
    if after != crashed:
        raise CheckFailed(f"move accepted after the collision: {after}")
    steps.append(dict(key=ignored, before=vars(crashed), after=vars(after)))
    press("r", lambda b: b.lane == 1 and b.finish == "playing", "restart")
    return moves


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--package", type=Path, default=ROOT / "build/lanes", help="built Lanes package")
    parser.add_argument("--output", type=Path, required=True, help="JSON record to write")
    parser.add_argument("--adapter", help="pass --adapter VVVV:DDDD to the game")
    parser.add_argument("--backend", choices=("proton", "wine"), default="proton")
    parser.add_argument("--proton", help="installed Valve or GE-Proton directory")
    parser.add_argument("--prefix", type=Path, required=True, help="Linux prefix for this check")
    parser.add_argument("--startup-timeout", type=float, default=120.0)
    args = parser.parse_args()
    package, prefix = args.package.resolve(), args.prefix.expanduser().resolve()
    missing = [name for name in PACKAGE if not (package / name).is_file()]
    if missing:
        parser.error(f"{package} lacks {', '.join(missing)}; run python build.py lanes")
    binaries = sorted(path for path in package.iterdir() if path.suffix.lower() in (".exe", ".dll"))
    record: dict = dict(schema=1, date_utc=datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
                        adapter=args.adapter, backend=args.backend,
                        package_sha256={path.name: digest(path) for path in binaries}, steps=[])
    started = time.monotonic()
    game = display = log = None
    with tempfile.TemporaryDirectory(prefix="lanes-input-") as work:
        on_wayland = bool(os.environ.get("WAYLAND_DISPLAY"))
        if on_wayland:
            server = private_x_server(work, geometry="1280x800")
        elif os.environ.get("DISPLAY") and screen_capture_problem() is None:
            server = nullcontext((os.environ["DISPLAY"], os.environ.get("XAUTHORITY")))
        else:
            server = nullcontext(None)
        with server as x:
            try:
                if not x:
                    raise CheckFailed("no readable X display: start Xwayland, or run under xvfb-run")
                record["display"] = "private rootful Xwayland" if on_wayland else "current X display"
                display = Display(*x)
                env = {k: v for k, v in os.environ.items() if k != "WAYLAND_DISPLAY"}
                env.update(DISPLAY=x[0], UMU_RUNTIME_UPDATE="0")
                if x[1]:
                    env["XAUTHORITY"] = x[1]
                command = [sys.executable, str(ROOT / "tuxbox.py"), "run", "--backend", args.backend,
                           "--prefix", str(prefix), *(["--proton", args.proton] if args.proton else []),
                           "--app-id", "Lanes", str(package), *(["--adapter", args.adapter] if args.adapter else [])]
                log = open(Path(work) / "launcher.log", "wb")  # noqa: SIM115 - closed in finally
                game = subprocess.Popen(command, cwd=ROOT, env=env, stdin=subprocess.DEVNULL, stdout=log,
                                        stderr=subprocess.STDOUT, start_new_session=True)
                deadline = time.monotonic() + args.startup_timeout
                found = None
                while not found and time.monotonic() < deadline and game.poll() is None:
                    found = display.find_window("lanes")
                    time.sleep(0.25)
                if not found:
                    raise CheckFailed("the lanes window did not appear" if game.poll() is None
                                      else f"the launcher exited with {game.returncode} before the window appeared")
                window, *origin = found
                record["focused"] = display.focus(window)
                moves = play(display, window, origin, record["steps"], game)
                display.press("Escape")
                try:
                    code = game.wait(timeout=30)
                except subprocess.TimeoutExpired:
                    raise CheckFailed("the game did not close after Escape") from None
                if code != 0:
                    raise CheckFailed(f"Escape closed the game with exit status {code}")
                # The launcher returns once the wineserver is gone; allow a moment for stragglers.
                deadline = time.monotonic() + 5
                while (stray := prefix_processes(prefix)) and time.monotonic() < deadline:
                    time.sleep(0.25)
                if stray:
                    raise CheckFailed(f"processes left in the prefix after exit: {stray}")
                record["receipt"] = (f"lanes-input PASS moves={moves} collision=1 ignored-after-collision=1 "
                                     f"restart=1 escape-exit=0 stray=0 input=xtest")
            except (CheckFailed, OSError, ValueError) as error:
                record["error"] = str(error)
            finally:
                if game and game.poll() is None:
                    os.killpg(game.pid, signal.SIGTERM)
                    try:
                        game.wait(timeout=10)
                    except subprocess.TimeoutExpired:
                        os.killpg(game.pid, signal.SIGKILL)
                        game.wait()
                if display:
                    display.close()
                if log:
                    log.close()
                if "error" in record:
                    leftovers = prefix_processes(prefix)
                    for pid in leftovers:
                        try:
                            os.kill(pid, signal.SIGKILL)
                        except OSError:
                            pass
                    record["killed_leftovers"] = leftovers
                    launcher_log = Path(work) / "launcher.log"
                    if launcher_log.is_file():
                        record["launcher_log_tail"] = launcher_log.read_text(errors="replace").splitlines()[-40:]
    record["seconds"] = round(time.monotonic() - started, 1)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(record, indent=2) + "\n")
    print(record.get("receipt") or f"lanes-input FAIL {record['error']}")
    return 0 if "receipt" in record else 1


if __name__ == "__main__":
    sys.exit(main())
