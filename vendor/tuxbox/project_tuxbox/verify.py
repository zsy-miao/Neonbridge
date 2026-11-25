"""Behavioral gates for the project-owned guest fixtures."""
from contextlib import contextmanager, nullcontext
import ctypes
import ctypes.util
import hashlib
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import platform
import select
import statistics
import re
import shutil
import struct
import sys
import subprocess
import tempfile

from .inspect import digest, export_table, image, launch_target

ROOT = Path(__file__).resolve().parents[1]
BUILD = ROOT / "build"
BASE_SUITES = ("cpu", "xmem", "xapu", "xgameruntime", "xcrdapi", "graphics", "graphics-abi", "guest-frame", "present", "lanes", "lanes-selftest", "lanes-shader-cache", "lanes-benchmark",
               "cpu-benchmark", "d3d12-exports", "d3d12-features", "d3d12-indirect", "d3d12-queries", "d3d12-queues", "d3d12-tables",
               "d3d12-heaps", "d3d12-placement", "d3d12-copy-resource", "d3d12-draw", "d3d12-stream", "d3d12-frame-events",
               "d3d12-guids", "d3d12-signatures", "xg-provider")
OPTIONAL_PROVIDERS = {"dxc-utils": "dxc-utils/dxcompiler.dll", "xg": "xg/xg_xs.dll"}
SUITES = (*BASE_SUITES, "d3d12-mesh", *OPTIONAL_PROVIDERS)
# The scenarios of `lanes.exe --selftest`, one gate each.
LANES_SCENARIOS = ("labels", "cross-queue", "compute-frame", "skipped-command", "copy-memory", "shared-texture",
                   "indirect-dispatch", "texture-readback",
                   "ray-tracing-builder", "thread-dimensions",
                   "descriptor-reclaim")


class LanesShader:
    """The Lanes console shader in a prefix's translation cache."""

    def __init__(self, host):
        from examples.lanes.shader_fixture import shader
        from .shader_prep import SOURCE_CACHE

        self.data = shader()
        self.key = hashlib.sha256(self.data).hexdigest()
        self.sources = Path(host.env["WINEPREFIX"]) / SOURCE_CACHE
        self.root = self.sources.parent

    def forget(self):
        # Only this synthetic shader's entries; a prefix may hold a title cache.
        for path in [*self.sources.glob(f"{self.key}.*"), *self.root.glob(f"runtime/*/{self.key}.*")]:
            path.unlink()

    def prepare(self, work):
        """Translate it into the prepared cache, as shader preparation does."""
        from .runtime_shaders import compile_request
        from .shader_prep import compiler_tools

        self.forget()
        self.sources.mkdir(parents=True, exist_ok=True)
        request = work / f"{self.key}.dxbc"
        request.write_bytes(self.data)
        compile_request(request, self.key, compiler_tools(), self.sources, None)


def lanes_shader_cache(host, check, gpu_args, pci, memory_symbols, work):
    """Load the Lanes console shader from a prepared cache, then from the runtime worker.

    The prepared gate translates it before launch, as shader preparation does.
    The runtime gates start with it uncached: the first launch must have the
    host worker compile it, and the second must reuse that result.
    """
    from .runtime_shaders import shader_session

    lanes = LanesShader(host)
    key, root, forget = lanes.key, lanes.root, lanes.forget

    saved = {name: host.env.get(name) for name in ("TUXBOX_LOG", "WINEDEBUG", "TUXBOX_SHADER_SESSION")}
    host.env.update(TUXBOX_LOG="1", WINEDEBUG="-all,+debugstr")
    host.env.pop("TUXBOX_SHADER_SESSION", None)
    loaded = f"prepared compute shader loaded, source SHA-256 {key}"

    def expect_loaded(output):
        return None if loaded in output else "the provider did not load the translated shader"

    def launch(name, log_check):
        return check(name, "lanes/lanes.exe", ["--selftest", "xbox-shader", *gpu_args, "--report"],
                     rf"lanes-selftest PASS pci={pci} scenario=xbox-shader passed=1",
                     providers={"lanes/xmem.dll": memory_symbols, "lanes/GameInput.dll": ("#1",),
                                "lanes/d3d12_xs.dll": ("D3D12XboxCreateDevice",)},
                     config="lanes/MicrosoftGame.config", device_factory="xbox", log_check=log_check)

    try:
        lanes.prepare(work)
        launch("lanes-shader-cache-prepared", expect_loaded)

        forget()
        for name, compiles in (("lanes-shader-cache-compiled", 1), ("lanes-shader-cache-reused", 0)):
            messages = []
            with shader_session(root, host.env, messages.append):
                launch(name, lambda output, messages=messages, compiles=compiles: expect_loaded(output) or (
                    None if sum(f"Compiled Xbox shader {key}" in line for line in messages) == compiles
                    else f"expected {compiles} worker compiles, saw: {messages}"))
        forget()
    finally:
        for name, value in saved.items():
            if value is None:
                host.env.pop(name, None)
            else:
                host.env[name] = value


def cpu_model():
    if sys.platform == "linux" and Path("/proc/cpuinfo").exists():
        for line in Path("/proc/cpuinfo").read_text().splitlines():
            if line.startswith("model name") and ":" in line:
                return line.partition(":")[2].strip()
    return platform.processor() or platform.machine()


def guid_receipt(imports):
    symbols = next((module["symbols"] for module in imports if module["module"].lower() == "d3d12_xs.dll"), ())
    count = sum(name.startswith(("IID_", "WKPDID_")) for name in symbols)
    return f"d3d12-guids PASS identifiers={count} bytes={16 * count}"


def lanes_imports(pe):
    """Check the title boundary, including imports that are resolved on first use."""
    modules = sorted({entry["module"].lower() for key in ("imports", "delay_imports") for entry in pe[key]})
    providers = {"d3d12_xs.dll", "gameinput.dll", "xmem.dll"}
    unexpected = [name for name in modules if name not in providers and not (
        name.startswith(("api-ms-win-core-", "api-ms-win-crt-")) and name.endswith(".dll"))]
    if unexpected:
        raise ValueError(f"Lanes imports a library outside its Xbox title profile: {', '.join(unexpected)}")
    return modules


ROOTLESS_X = ("the X server refuses to read its root window (rootless XWayland), so screen samples "
              "read 0; run the present suite under xvfb-run or a rootful Xwayland")


def xauthority_entry(cookie):
    """One MIT-MAGIC-COOKIE-1 Xauthority record that matches any host and display number."""
    fields = (b"", b"", b"MIT-MAGIC-COOKIE-1", cookie)  # FamilyWild: empty address and number
    return struct.pack(">H", 0xFFFF) + b"".join(struct.pack(">H", len(f)) + f for f in fields)


def _free_display_number(start=2, attempts=16):
    """Pick a display number free of lock files and sockets, or None."""
    for candidate in range(start, start + attempts):
        if Path(f"/tmp/.X{candidate}-lock").exists():
            continue
        if Path(f"/tmp/.X11-unix/X{candidate}").exists():
            continue
        return candidate
    return None


@contextmanager
def private_x_server(directory, geometry="2560x1440", timeout=10):
    """Yield (DISPLAY, XAUTHORITY) for a temporary rootful Xwayland, or None if one can't start.

    The rootful server draws into its own Wayland window and keeps a readable root window, so
    Wine's screen samples work on a Wayland desktop. It accepts only its own random cookie,
    is started on a display number picked below the desktop's and is stopped when the block exits.

    Xwayland takes its display number as the positional argument (":N"); "-displayfd" only
    reports the number it took, so a free one is chosen here instead of trusting the default,
    which is the inherited DISPLAY and collides with the desktop server.
    """
    xwayland = shutil.which("Xwayland")
    if not sys.platform.startswith("linux") or not xwayland or not os.environ.get("WAYLAND_DISPLAY"):
        yield None
        return
    display = _free_display_number()
    if display is None:
        yield None
        return
    auth = Path(directory) / "Xauthority"
    descriptor = os.open(auth, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(descriptor, "wb") as stream:
        stream.write(xauthority_entry(os.urandom(16)))
    read, write = os.pipe()
    try:
        server = subprocess.Popen([xwayland, f":{display}", "-displayfd", str(write), "-geometry", geometry,
                                   "-noreset", "-auth", str(auth)], pass_fds=(write,), stdin=subprocess.DEVNULL,
                                  stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    except OSError:
        os.close(read)
        os.close(write)
        yield None
        return
    os.close(write)
    try:
        number = b""
        while not number.endswith(b"\n") and select.select([read], [], [], timeout)[0]:
            chunk = os.read(read, 16)
            if not chunk:
                break
            number += chunk
        # A server that exits before being ready is not a display we can use.
        if number.strip().isdigit() and server.poll() is None:
            yield (f":{int(number)}", str(auth))
        else:
            yield None
    finally:
        os.close(read)
        server.terminate()
        try:
            server.wait(timeout=5)
        except subprocess.TimeoutExpired:
            server.kill()
            server.wait()


def screen_capture_problem(display=None, library=None, xauthority=None):
    """Why the present checks cannot sample the screen on this host, or None.

    Wine backs GetDC(nullptr)/GetPixel with the X root window. The rootless XWayland of a
    Plasma or GNOME Wayland session rejects GetImage on that window with BadMatch, so every
    sample reads 0 even though the frames are presented. Only that refusal is reported;
    anything else is left for the guest check to report.
    """
    display = os.environ.get("DISPLAY") if display is None else display
    if not sys.platform.startswith("linux") or not display:
        return None
    try:
        x11 = ctypes.CDLL(library or ctypes.util.find_library("X11") or "libX11.so.6")
    except OSError:
        return None
    x11.XOpenDisplay.restype = ctypes.c_void_p
    x11.XOpenDisplay.argtypes = [ctypes.c_char_p]
    x11.XDefaultRootWindow.restype = ctypes.c_ulong
    x11.XDefaultRootWindow.argtypes = [ctypes.c_void_p]
    x11.XGetImage.restype = ctypes.c_void_p
    x11.XGetImage.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_int, ctypes.c_int,
                              ctypes.c_uint, ctypes.c_uint, ctypes.c_ulong, ctypes.c_int]
    x11.XDestroyImage.argtypes = [ctypes.c_void_p]
    x11.XCloseDisplay.argtypes = [ctypes.c_void_p]
    handler_type = ctypes.CFUNCTYPE(ctypes.c_int, ctypes.c_void_p, ctypes.c_void_p)
    x11.XSetErrorHandler.restype = ctypes.c_void_p
    x11.XSetErrorHandler.argtypes = [handler_type]
    errors = []
    # Xlib's default handler exits the process; record the error instead.
    handler = handler_type(lambda _display, _event: errors.append(1) or 0)
    saved = os.environ.get("XAUTHORITY")
    if xauthority is not None:
        os.environ["XAUTHORITY"] = xauthority  # Xlib reads the cookie file when it connects
    try:
        connection = x11.XOpenDisplay(display.encode())
    finally:
        if xauthority is not None:
            if saved is None:
                os.environ.pop("XAUTHORITY", None)
            else:
                os.environ["XAUTHORITY"] = saved
    if not connection:
        return None
    previous = x11.XSetErrorHandler(handler)
    try:
        image = x11.XGetImage(connection, x11.XDefaultRootWindow(connection), 0, 0, 1, 1,
                              0xFFFFFFFF, 2)  # ZPixmap
        if image:
            x11.XDestroyImage(image)
    finally:
        x11.XCloseDisplay(connection)
        x11.XSetErrorHandler(ctypes.cast(previous, handler_type) if previous else handler_type())
    return ROOTLESS_X if errors else None


def missing_provider_imports(modules, provider, expected):
    """Return missing expected static imports, resolving #ordinal imports via provider exports."""
    found = {symbol for mod in modules if mod["module"].lower() == provider.name.lower()
             for symbol in mod["symbols"]}
    ordinals = [int(symbol[1:]) for symbol in found if symbol.startswith("#") and symbol[1:].isdigit()]
    if ordinals and not set(expected).issubset(found):
        slots, _ = export_table(provider)
        for ordinal in ordinals:
            found.update(slots.get(ordinal, ()))
    return sorted(set(expected) - found)


def verify(host, output, adapter, selected=None, samples=1):
    if adapter and not re.fullmatch(r"[0-9a-fA-F]{4}:[0-9a-fA-F]{4}", adapter):
        raise ValueError("Adapter must be a PCI vendor:device pair in hexadecimal")
    if not selected:
        selected = (*BASE_SUITES, *(suite for suite, provider in OPTIONAL_PROVIDERS.items()
                                   if (BUILD / provider).is_file()))
    cases = []
    builds = {}
    present_display = None
    BUILD.mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="guest-check-", dir=BUILD) as tmp:
        work = Path(tmp)

        def stage(relative, folder):
            src = BUILD / relative
            if not src.is_file():
                raise ValueError(f"Build the missing fixture first: {src}")
            dest = folder / src.name
            if not dest.exists():
                shutil.copy2(src, dest)
                builds[relative] = digest(dest)
            return dest

        def check(name, binary, arguments, match, expected_code=0, providers=None, config=None, device_factory=None,
                  dependencies=(), log_check=None):
            row = {"gate": name, "passed": False}
            cases.append(row)
            folder = work / str(len(cases))
            folder.mkdir()
            receipt = folder / "result.txt"
            try:
                exe = stage(binary, folder)
                if providers:
                    pe = image(exe)
                    modules = pe["imports"]
                    if config:
                        row["guest_import_modules"] = lanes_imports(pe)
                    if callable(match):
                        match = match(modules)
                    if device_factory is not None:
                        desktop = next((entry["symbols"] for entry in modules
                                        if entry["module"].lower() == "d3d12.dll"), ())
                        if device_factory == "xbox" and desktop:
                            raise ValueError("Frame fixture imports desktop D3D12 directly")
                        # The Windows SDK import library selects this function by ordinal.
                        if device_factory == "desktop" and not {"D3D12CreateDevice", "#101"}.intersection(desktop):
                            raise ValueError("Expected device creation from desktop d3d12.dll")
                    for dll, symbols in providers.items():
                        provider = stage(dll, folder)
                        missing = missing_provider_imports(modules, provider, symbols)
                        if missing:
                            raise ValueError(f"Missing static guest imports from {provider.name}: {missing}")
                for dependency in dependencies:
                    stage(dependency, folder)
                if config:
                    stage(config, folder)
                    exe = launch_target(folder, "Lanes")
                else:
                    exe = launch_target(exe, None)
                proc = host.run(exe, [*arguments, receipt.name], timeout=15, capture=True)
                text = receipt.read_text(encoding="utf-8").rstrip("\r\n") if receipt.is_file() else None
                row.update(exit_code=proc.returncode, receipt=text)
                row["passed"] = (proc.returncode == expected_code and
                                 (text is None if match is None else text is not None and re.fullmatch(match, text) is not None))
                if row["passed"] and log_check:
                    problem = log_check(proc.stdout.decode(errors="replace"))
                    if problem:
                        row.update(passed=False, error=problem)
                if not row["passed"]:
                    print(proc.stdout[-2000:].decode(errors="replace"))
            except (OSError, ValueError, subprocess.TimeoutExpired) as error:
                row["error"] = str(error)
            print(f"{'PASS' if row['passed'] else 'FAIL'}  {name}", flush=True)
            return row

        def unsampled(name, reason):
            row = {"gate": name, "passed": False, "error": f"screen capture unavailable: {reason}"}
            cases.append(row)
            print(f"FAIL  {name}  ({row['error']})", flush=True)
            return row

        if "cpu" in selected:
            for route, state in [("RRRRDD", "coin_mask=1 ticks=6 bumps=0"), ("LDDRRRR", "coin_mask=2 ticks=7 bumps=1")]:
                check(f"route-{route}", "route-game.exe", [route], f"route-game x=5 y=3 {state} exit=1")
            check("invalid-route", "route-game.exe", ["X"], None, 2)
        gpu_args = ["--adapter", adapter] if adapter else []
        pci = adapter.lower() if adapter else r"[0-9a-f]{4}:[0-9a-f]{4}"
        memory_symbols = ("XMemAllocDefault", "XMemFreeDefault")
        graphics_symbols = ("D3D12SerializeRootSignature",)
        if "dxc-utils" in selected:
            binary = "dxc-utils/dxc-utils.exe"
            provider = "dxc-utils/dxcompiler_xs.dll"
            dependencies = ("dxc-utils/dxcompiler.dll",)
            check("dxc-create-blob", binary, ["--report"],
                  r"dxc-utils PASS bytes=257 source-freed=1 writable=257 retained=1",
                  providers={provider: ("DxcCreateInstance",)}, dependencies=dependencies)
            check("dxc-allocator", binary, ["--allocator", "--report"],
                  "dxc-allocator PASS ordinals=3,4 custom=1 retained=1 balanced=1",
                  providers={provider: ("DxcCreateInstance2",)}, dependencies=dependencies)
            check("dxc-compile", binary, ["--compile", "--report"],
                  r"dxc-compiler PASS profile=cs_6_0 dxil=1 invalid-entry=1 diagnostics=1",
                  providers={provider: ("DxcCreateInstance",)}, dependencies=dependencies)
            check("xdxr-link", "dxc-utils/xdxr-link.exe", ["--report"],
                  r"xdxr-link PASS factories=2 flat=1 grouped=1 retained-shader-body=1 shader-tables=single,multiple",
                  providers={"dxc-utils/dxcompiler_xs.dll": ("CreateXdxrCompiler", "CreateXdxrCompiler2", "DxcCreateInstance")},
                  dependencies=("dxc-utils/dxcompiler.dll",))
        xg_addresses = r"xg-texture PASS layouts=18 traced-descriptors=8 element-addresses=872759"
        if "d3d12-signatures" in selected:
            check("d3d12-signatures", "d3d12-smoke/d3d12-signatures.exe", ["--report"],
                  r"d3d12-signatures PASS versions=1.0,1.1 ranges=1 invalid=2",
                  providers={"d3d12-smoke/d3d12_xs.dll": ("D3D12SerializeVersionedRootSignature",
                             "D3D12CreateRootSignatureDeserializer", "D3D12CreateVersionedRootSignatureDeserializer")})
        if "d3d12-guids" in selected:
            check("d3d12-guids", "d3d12-smoke/d3d12-guids.exe", ["--report"],
                  guid_receipt,
                  providers={"d3d12-smoke/d3d12_xs.dll": ("IID_ID3D12Device",)})
        if "xg-provider" in selected:
            check("xg-dcc", "xg-provider/xg-dcc.exe", ["--report"],
                  r"xg-dcc PASS xbox-blocks=2 encode-bytes=32 atomic-batch=1",
                  providers={"xg-provider/xg_xs.dll": ("XGEncodeDCC", "XGDecodeDCC")})
            check("xg-library", "xg-provider/xg-library.exe", ["--report"],
                  r"xg-library PASS depth=516 descriptors=375 lock=recursive\+factory copies=bidirectional depth_surfaces=16 typed_images=880 map_targets=1474 msaa_depth=384",
                  providers={"xg-provider/xg_xs.dll": ("XGCreateTextureComputerFromDescriptor",
                             "XGInitializeLibrary", "XGTerminateLibrary", "XGSetHardwareVersion",
                             "XGEnableExclusiveLocking", "XGEnterExclusiveLock", "XGLeaveExclusiveLock",
                             "XGComputeOptimalDepthStencilSwizzleMode")})
        if "xg" in selected:
            check("xg-native-layout", "xg/xg-texture.exe", ["--report"],
                  r"xg-texture PASS descriptors=4 layout-queries=8 plane-records=4 releases=4",
                  providers={"xg/xg_xs.dll": ("XGCreateTextureComputer",)},
                  dependencies=("xmem-smoke/xmem.dll",))
        for suite, addresses_gate, formats_gate, deps in (
            ("xg", "xg-native-addresses", "xg-native-formats", ("xmem-smoke/xmem.dll",)),
            ("xg-provider", "xg-addresses", "xg-formats", ()),
        ):
            if suite in selected:
                provider = {f"{suite}/xg_xs.dll": ("XGCreateTextureComputer",)}
                check(addresses_gate, f"{suite}/xg-texture.exe", ["--addresses", "--report"], xg_addresses,
                      providers=provider, dependencies=deps)
                check(formats_gate, f"{suite}/xg-texture.exe", ["--formats", "--report"],
                      r"xg-formats PASS formats=78 layouts=156 element-addresses=9231018",
                      providers=provider, dependencies=deps)
        if "xgameruntime" in selected:
            check("xgameruntime", "xgameruntime/runtime-control.exe", [],
                  r"xgameruntime PASS init=balanced interfaces=checked manual=ordered termination=canceled automatic=3 stale=rejected",
                  providers={"xgameruntime/xgameruntime.dll": ("InitializeApiImpl", "InitializeApiImplEx",
                                                               "UninitializeApiImpl", "QueryApiImpl")})
        if "xapu" in selected:
            check("xapu", "xapu/xapu-control.exe", [],
                  r"xapu PASS Opus=960 pre-skip=312 output=648 queue=ordered",
                  providers={"xapu/xapu.dll": ("XApuConnect", "XApuEnqueueCommand", "XApuDequeueResult")})
        if "xcrdapi" in selected:
            check("xcrdapi", "xcrdapi/xcrdapi-read.exe", [],
                  r"raw=16 zlib=16 guards=2 errors=2 cancellation=1 autosubmit=64 PASS",
                  providers={"xcrdapi/xcrdapi.dll": ("DStorageGetFactory",)})
        if "xmem" in selected:
            check("xmem-heaps", "xmem-smoke/xmem-smoke.exe", ["--heaps", "--report"],
                  r"xmem-heaps PASS title=1 private=4 invalid=2 pool-commits=2 pool-decommits=2 heap-callbacks=2 stats=1 dump=1 history=2 page-classes=3",
                  providers={"xmem-smoke/xmem.dll": ("XMemGetTitleHeap", "XMemCreateWin32Heap", "XMemAllocDefault", "XMemFreeDefault")})
            check("xmem-heap-profiles", "xmem-smoke/xmem-smoke.exe", ["--heap-profiles", "--report"],
                  r"xmem-heap-profiles PASS attributes=40 alloc=80 free=80 null-free=2",
                  providers={"xmem-smoke/xmem.dll": ("XMemAlloc", "XMemFree", "XMemAllocDefault", "XMemFreeDefault")})
            check("xmem-virtual-cpu", "xmem-smoke/xmem-smoke.exe", ["--virtual-cpu", "--report"],
                  r"xmem-virtual-cpu PASS protections=6 allocations=12 guards=5 invalid=1",
                  providers={"xmem-smoke/xmem.dll": ("XMemVirtualAlloc",)})
            check("xmem-scatter", "xmem-smoke/xmem-smoke.exe", ["--scatter", "--report"],
                  r"xmem-scatter PASS pages=3 remapped-bytes=196608 duplicates=2 guards=2 rollback=1",
                  providers={"xmem-smoke/xmem.dll": ("XMemMapPhysicalPagesScatter", "XMemAllocatePhysicalPages", "XMemFreePhysicalPages")})
            check("xmem-watch", "xmem-smoke/xmem-smoke.exe", ["--watch", "--report"],
                  r"xmem-watch PASS initial=2 reset=2 next=1 guards=2 invalid=1 pause=2 resume=3 allocations=2 direct-tags=2 retail-apertures=2",
                  providers={"xmem-smoke/xmem.dll": ("XMemVirtualAlloc", "XMemGetWriteWatch", "XMemResumeWriteWatch", "XMemSetOption")})
            check("xmem-hooks", "xmem-smoke/xmem-smoke.exe", ["--hooks", "--report"],
                  r"xmem-hooks PASS calls=2/2 zero-size=1 null-free=1 restore=1 reentrant=1",
                  providers={"xmem-smoke/xmem.dll": ("XMemAlloc", "XMemFree", "XMemSetAllocationHooks")})
            check("xmem-tags", "xmem-smoke/xmem-smoke.exe", ["--tags", "--report"],
                  r"xmem-tags PASS count=5120 hash=d4bf30c57509b176",
                  providers={"xmem-smoke/xmem.dll": ("XMemMakeTag",)})
            check("xmem-http", "xmem-smoke/xmem-http.exe", ["--report"],
                  r"xmem-http PASS callbacks=7 host-session=1",
                  providers={"xmem-smoke/xmem.dll": ("XMemSetWinHttpHooks",),
                             "xmem-smoke/tuxbox_xhttp.dll": (
                                 "WinHttpOpen", "WinHttpConnect", "WinHttpOpenRequest",
                                 "WinHttpWebSocketCompleteUpgrade", "WinHttpCloseHandle",
                                 "WinHttpReceiveResponse", "WinHttpSetOption")})
            check("xmem-query", "xmem-smoke/xmem-smoke.exe", ["--query", "--report"],
                  r"xmem-query PASS cpu=5 released=1 unsupported=3 guards=2 null=1",
                  providers={"xmem-smoke/xmem.dll": ("XMemVirtualQuery",)})
            check("xmem-default", "xmem-smoke/xmem-smoke.exe", ["--report"],
                  re.escape("xmem-default: PASS (allocator pairs 1/6 and 2/7, ordinary and 64 KiB allocations, "
                            "host working set v2, virtual reserve/commit)"),
                  providers={"xmem-smoke/xmem.dll": (*memory_symbols, "XMemAlloc", "XMemFree",
                                                     "XMemGetWorkingSetStatistics", "XMemVirtualAlloc")})
            check("xmem-partitions", "xmem-smoke/xmem-partitions.exe", ["--report"],
                  r"xmem-partitions PASS stable_cpu_pfns=2 budget_reduced=1 ids=2 native_handles=[0-2]",
                  providers={"xmem-smoke/xmem.dll": ("XMemGetPartitionHandleAndId",
                             "XMemGetPhysicalMemoryLayout", "XMemTransferMemory")})
            check("xmem-pages", "xmem-smoke/xmem-smoke.exe", ["--pages", "--report"],
                  r"xmem-pages PASS cpu-pages=2 remapped-bytes=131072 stale-and-foreign-rejected=1 shared-aliases=3 scatter-aliases=2 rollback=1 free-unmaps=2",
                  providers={"xmem-smoke/xmem.dll": ("XMemAllocatePhysicalPages", "XMemMapPhysicalPages",
                                                     "XMemFreePhysicalPages")})
        if "graphics" in selected:
            check("desktop-clear", "graphics/desktop-clear.exe", [*gpu_args, "--report"],
                  rf"desktop-clear PASS adapter=.+ pci={pci} pixels=481/481 row_bytes=148 row_pitch=[1-9][0-9]*")
        if "graphics-abi" in selected:
            check("root-signature", "d3d12-smoke/d3d12-root-smoke.exe", [*gpu_args, "--report"],
                  rf"d3d12-root-smoke PASS import=D3D12SerializeRootSignature pci={pci} "
                  r"constant=b5/space2 uav=u3/space4 dispatches=2 words=64/64 guards=16",
                  providers={"d3d12-smoke/d3d12_xs.dll": (*graphics_symbols, "D3D12CreateRootSignatureDeserializer")},
                  device_factory="desktop")
        frame_match = (rf"xbox-frame PASS pci={pci} submissions=2 snapshots=2 pixels=962/962 "
                       r"inset=208 guards=3 fence=2 compute_fence=7")
        frame_provider = {"d3d12-smoke/d3d12_xs.dll": ("D3D12XboxCreateDevice",)}
        if "d3d12-exports" in selected:
            dll = "d3d12-smoke/d3d12_xs.dll"

            def exports_receipt(_imports):
                slots, _ = export_table(BUILD / dll)
                cpu_clear = any("D3D12ZeroMemoryX" in names for names in slots.values())
                return "d3d12-exports PASS debug=1 blob=1 dxgix=4" + (" cpu_clear=1" if cpu_clear else "")

            check("d3d12-exports", "d3d12-smoke/d3d12-exports.exe", ["--report"],
                  exports_receipt, providers={dll: ("D3D10CreateBlob", "D3D12XboxGetProcessDebugFlags",
                                                   "D3D12XboxSetProcessDebugFlags")})
        for suite, expected in (
            ("features", r"queries=7 binding=[1-3] heap=[12] root=1 dxr=0 allocations=4 footprints=4"),
            ("heaps", r"heap=32MiB offsets=0,65536 words=256/256 invalid=4 display=3840x2160/RGB10A2 pixel=c00ffc00 volume=45/45 priorities=4/4 invalid_priority_calls=4"),
            ("placement", r"slot=44 VA-offsets=0,4096,0 passes=2 BC1-blocks=522 cube-blocks=16404 R8-texels=262144 " +
             ("rejected=4 configured_vm=1" if host.env.get("TUXBOX_TEST_CONFIGURED_VM") else "rejected=3 configured_vm=0")),
            ("copy-resource", r"slot=84 buffer_bytes=1028 texture_subresources=8 nonzero_option=E_NOTIMPL "
                              r"self_copy=E_INVALIDARG source_and_destination=verified placed_uploads=2 placed_dst=verified "
                              r"same_va_alias=verified cpu_writeback=verified partial_copy_texels=2 overlap_mips=4 pending_reader=verified"),
            ("draw", r"red=36 blue=220"),
            ("mesh", r"tier=1 stream=680 direct=6 amplified=12 indirect=2 zero=0 pixels=256/256 guards=32 rejected=5"),
            ("frame-events", r"slots=78,79,80,81 tokens=2 timeout=S_FALSE flip=1 detached=1 stats=2"),
            ("tables", r"words=16/16 guards=4 heaps=2 rejected=3 placed-srv=16/16 range-copies=4 descriptor-mode=1"),
            ("queues", r"queues=3 bytes=64 wait=1 signal-x=2"),
            ("queries", r"timestamps=2 occlusion=0 guards=2 heaps=6 rejected=3"),
            ("indirect", r"sdk=0x20004 device=41 list=59 words=64/64 counts=direct,one,zero flush=global,range,wake-up\(0x3000:0\+0xa30000\),zero-length,global-range budget=default,rejected2 mesh=0x[0-9a-f]{8} rays=0x[0-9a-f]{8}"),
        ):
            name = f"d3d12-{suite}"
            if name in selected:
                check(name, f"d3d12-smoke/{name}.exe", [*gpu_args, "--report"],
                      rf"{name} PASS pci={pci} {expected}", providers=frame_provider, device_factory="xbox")
        if "d3d12-stream" in selected:
            check("d3d12-stream", "d3d12-smoke/d3d12-draw.exe", ["--stream", *gpu_args, "--report"],
                  rf"d3d12-stream PASS pci={pci} slot=86 bytes=712 indices=16,32 half-red-blue=18 magenta=18 blue=220 rejected=4",
                  providers=frame_provider, device_factory="xbox")
        if "guest-frame" in selected:
            check("guest-factories", "d3d12-smoke/d3d12-factory.exe", [*gpu_args, "--report"],
                  rf"xbox-factory PASS pci={pci} adapter={'selected' if adapter else 'default'} "
                  r"profile=0x20004 queues=2 fences=2 flag-masks=4 dxgi=2",
                  providers={"d3d12-smoke/d3d12_xs.dll": ("D3D12XboxCreateDevice", "D3D12CreateDevice",
                             "D3D12XboxSetProcessDebugFlags", "D3D12XboxGetProcessDebugFlags")}, device_factory="xbox")
            check("guest-command-list", "d3d12-smoke/xbox-frame.exe", [*gpu_args, "--report"],
                  frame_match, providers=frame_provider, device_factory="xbox")
            check("guest-frame-rgb10", "d3d12-smoke/xbox-frame-rgb10.exe", [*gpu_args, "--report"],
                  frame_match + r" format=rgb10a2", providers=frame_provider, device_factory="xbox")
            check("guest-command-errors", "d3d12-smoke/xbox-frame-negative.exe", [*gpu_args, "--report"],
                  rf"xbox-frame-negative PASS pci={pci} foreign-parents=2 copy-errors=2 recovered=1 rejected-batches=3 compute-present-rejected=1",
                  providers=frame_provider, device_factory="xbox")
        if "present" in selected:
            # The desktop's rootless XWayland can't be sampled; present into a private rootful
            # Xwayland instead, and fail fast with the cause if that isn't possible either.
            capture = screen_capture_problem()
            server = private_x_server(work) if capture else nullcontext()
            saved = {key: host.env.get(key) for key in ("DISPLAY", "XAUTHORITY")}
            with server as private:
                if private:
                    capture = screen_capture_problem(display=private[0], xauthority=private[1])
                    if capture is None:
                        host.env.update(DISPLAY=private[0], XAUTHORITY=private[1])
                        present_display = "private rootful Xwayland"
                        print(f"present: sampling a private rootful Xwayland on {private[0]}", flush=True)
                try:
                    present = check if capture is None else (lambda name, *_args, **_kwargs: unsampled(name, capture))
                    present("guest-present", "d3d12-smoke/xbox-frame.exe", [*gpu_args, "--present", "--report"],
                            frame_match + r" displayed=2/2", providers=frame_provider, device_factory="xbox")
                    present("guest-present-rgb10", "d3d12-smoke/xbox-frame-rgb10.exe", [*gpu_args, "--present", "--report"],
                            frame_match + r" format=rgb10a2 displayed=2/2", providers=frame_provider, device_factory="xbox")
                    for mode in ("owned", "oversized", "fit"):
                        present(f"guest-present-4k-{mode}", "d3d12-smoke/xbox-frame-4k.exe",
                                [mode, *gpu_args, "--present", "--report"],
                                rf"xbox-frame PASS pci={pci} submissions=2 snapshots=2 pixels=16588800/16588800 "
                                r"inset=921600 guards=3 fence=2 compute_fence=7 format=rgb10a2 displayed=2/2 "
                                r"client=\d+x\d+ origin=-?\d+,-?\d+ monitor=\d+x\d+", providers=frame_provider, device_factory="xbox")
                finally:
                    for key, value in saved.items():
                        if value is None:
                            host.env.pop(key, None)
                        else:
                            host.env[key] = value
        if "lanes" in selected:
            check("lanes-xbox-replay", "lanes/lanes.exe", ["--replay", *gpu_args, "--report"],
                  rf"lanes-replay PASS pci={pci} course=16/16 win=16/16 safe-miss=1 collision=1 frames=2 input=gameinput",
                  providers={"lanes/xmem.dll": memory_symbols, "lanes/GameInput.dll": ("#1",),
                             "lanes/d3d12_xs.dll": (*graphics_symbols, "D3D12XboxCreateDevice")},
                  config="lanes/MicrosoftGame.config", device_factory="xbox")
        if "lanes-selftest" in selected:
            for scenario in LANES_SCENARIOS:
                check(f"lanes-selftest-{scenario}", "lanes/lanes.exe",
                      ["--selftest", scenario, *gpu_args, "--report"],
                      rf"lanes-selftest PASS pci={pci} scenario={scenario} passed=1",
                      providers={"lanes/xmem.dll": memory_symbols, "lanes/GameInput.dll": ("#1",),
                                 "lanes/d3d12_xs.dll": ("D3D12XboxCreateDevice",)},
                      config="lanes/MicrosoftGame.config", device_factory="xbox",
                      dependencies=("lanes/XboxBvhBuilder.dll",) if scenario == "ray-tracing-builder" else ())
        if "lanes-benchmark" in selected:
            # Console shaders are only translated on the Proton backend, so the
            # pipeline readings exist there alone.
            shader = LanesShader(host) if host.backend == "proton" else None
            readings = rf"lanes-benchmark PASS pci={pci}(?: [a-z0-9_]+=[0-9]+\.[0-9])+"
            if shader:
                shader.prepare(work)
                readings = (rf"lanes-benchmark PASS pci={pci}(?: [a-z0-9_]+=[0-9]+\.[0-9])*"
                            r" shader_pipeline_us=[0-9]+\.[0-9] dxil_pipeline_us=[0-9]+\.[0-9]")
            try:
                check("lanes-benchmark", "lanes/lanes.exe", ["--benchmark", *gpu_args, "--report"], readings,
                      providers={"lanes/xmem.dll": memory_symbols, "lanes/GameInput.dll": ("#1",),
                                 "lanes/d3d12_xs.dll": ("D3D12XboxCreateDevice",)},
                      config="lanes/MicrosoftGame.config", device_factory="xbox")
            finally:
                if shader:
                    shader.forget()
        if "lanes-shader-cache" in selected and host.backend == "proton":
            lanes_shader_cache(host, check, gpu_args, pci, memory_symbols, work)
        if "cpu-benchmark" in selected:
            readings = []
            for _ in range(samples):
                sample = check("cpu-replay", "route-game.exe", ["--bench"],
                               r"route-bench routes=65536 moves=8388608 checksum=24316bf89788468d "
                               r"elapsed_ticks=[1-9][0-9]* frequency=[1-9][0-9]*")
                readings.append(sample)
            if samples > 1:
                del cases[-samples:]
                series = {"gate": "cpu-replay", "passed": all(row["passed"] for row in readings), "samples": readings}
                if series["passed"]:
                    seconds = []
                    for run in readings:
                        ticks, hz = (int(s.split("=")[1]) for s in run["receipt"].split()[-2:])
                        seconds.append(ticks / hz)
                    series["median_ms"] = round(statistics.median(seconds) * 1000, 4)
                cases.append(series)

    report = {"schema": 1, "type": "homebrew", "date_utc": datetime.now(timezone.utc).date().isoformat(),
              "host": platform.system(), "os_release": platform.release(), "cpu": cpu_model(),
              "machine": platform.machine(), "backend": host.backend,
              "provider": host.version, "binaries_sha256": builds, "checks": cases}
    if present_display:
        report["present_display"] = present_display
    output.parent.mkdir(parents=True, exist_ok=True)
    tmp_path = None
    try:
        with tempfile.NamedTemporaryFile(mode="w", encoding="utf-8", dir=output.parent, delete=False) as tmp:
            tmp_path = Path(tmp.name)
            json.dump(report, tmp, indent=2)
            tmp.write("\n")
        tmp_path.replace(output)
    finally:
        if tmp_path is not None and tmp_path.exists():
            try:
                tmp_path.unlink()
            except OSError:
                pass
    return int(any(not row["passed"] for row in cases))


GATE_LABELS = {
    "xapu": "Opus PCM, decoder pre-skip and command completion",
    "xgameruntime": "Game Runtime initialization, task delivery and cancellation",
    "xcrdapi": "Xbox storage file reads, zlib and queue completion",
    "guest-factories": "Xbox 0x20004 and default factories, compute queues and process debug flags",
    "route-RRRRDD": "Upper route and guest output", "route-LDDRRRR": "Lower route and collision",
    "xmem-pages": "CPU page allocation, remapping and release",
    "xmem-query": "CPU virtual memory and image queries",
    "xmem-tags": "Memory allocation tag encoding",
    "xmem-hooks": "Guest allocator callback routing",
    "xmem-watch": "Guest write-watch dirty-page query",
    "xmem-scatter": "CPU scattered physical-page mapping",
    "xmem-virtual-cpu": "CPU virtual-memory allocation profiles",
    "xmem-heap-profiles": "CPU heap attribute profiles",
    "xmem-heaps": "CPU title and private heap creation",
    "invalid-route": "Invalid input exit status", "xmem-default": "Xbox memory allocation and working set",
    "d3d12-signatures": "Root signature 1.0 and 1.1 serialization, deserialization and errors",
    "d3d12-exports": "Blob, debug interface, four DXGIX entry points and Series CPU memory clear",
    "d3d12-features": "Xbox capability records compared with the selected adapter",
    "d3d12-indirect": "Indirect dispatch and texture cache flushes between compute writes",
    "guest-frame-rgb10": "RGB10A2 display allocation and two guarded GPU readbacks",
    "guest-present-4k-owned": "4K RGB10 display scaled to a TuxBox window",
    "guest-present-4k-oversized": "4K RGB10 display fitted to the title window’s monitor",
    "guest-present-4k-fit": "4K RGB10 display scaled inside a smaller title window",
    "guest-present-rgb10": "Two RGB10A2 frames sampled from the desktop window",
    "d3d12-frame-events": "Frame origins, timeout tokens, completed presentation events and display statistics",
    "d3d12-heaps": "Placed resources and allocation offsets in a shared memory heap",
    "d3d12-placement": "Virtual-address BC1 cubes and R8 textures across page changes",
    "d3d12-copy-resource": "Whole-buffer and multi-mip texture copies through Xbox command-list slot 84",
    "d3d12-mesh": "Mesh and amplification shader dispatch on a mesh-tier adapter",
    "d3d12-draw": "Scissored triangle through Xbox graphics pipeline and draw slots",
    "d3d12-stream": "Xbox Device2 graphics pipeline stream, ownership and masked draw",
    "d3d12-tables": "Shader reads and writes through tables, including a late-copied texture view",
    "d3d12-queues": "Direct, compute and copy queue synchronization",
    "d3d12-queries": "GPU timestamps, occlusion readback and descriptor heap handles",
    "d3d12-guids": "D3D12 and DXGI identifier data by name and ordinal",
    "xg-native-layout": "Native Scarlett XG texture layouts with Rust XMem",
    "xg-native-addresses": "Supplied XG texture layouts and element addresses",
    "xg-native-formats": "Supplied XG layouts and addresses across 78 formats",
    "xg-formats": "TuxBox XG layouts and addresses across 78 formats",
    "xg-addresses": "TuxBox XG texture layouts and element addresses",
    "dxc-allocator": "Xbox DXC caller allocator and factory ordinals",
    "dxc-create-blob": "Xbox DXC utilities and binary blob lifetime",
    "dxc-compile": "Scarlett DXC compute shader and compiler diagnostics",
    "desktop-clear": "Desktop D3D12 pixel readback", "root-signature": "Xbox root-signature import and compute readback",
    "guest-command-list": "Xbox command-list clear and ordered copy readback",
    "guest-command-errors": "Guest command rejection, fence isolation and list recovery",
    "guest-present": "Xbox PresentX and two screen-verified frames",
    "lanes-xbox-replay": "Lanes Xbox imports, course, board readback and gameplay replay",
    "lanes-selftest-labels": "Bottom-of-pipe label writes with and without a flush",
    "lanes-selftest-cross-queue": "Compute and direct queues waiting on each other's labels",
    "lanes-selftest-compute-frame": "Compute queue frame submission",
    "lanes-selftest-skipped-command": "A skipped command leaving its list and fence write intact",
    "lanes-selftest-copy-memory": "CopyMemoryToMemoryX direction and contents",
    "lanes-selftest-shared-texture": "A waiting queue reading a texture another queue writes",
    "lanes-selftest-indirect-dispatch": "ExecuteIndirect matching a direct dispatch",
    "lanes-selftest-texture-readback": "A guest texture copied back to title memory",
    "lanes-benchmark": "Lanes frame, submission, fence handover, copy and pipeline timings",
    "lanes-selftest-ray-tracing-builder": "Single, copied and batched builds through XboxBvhBuilder",
    "lanes-selftest-thread-dimensions": "Slot 80 thread-dimension dispatch converted using the bound PSO group size",
    "lanes-selftest-descriptor-reclaim": "Shader-visible descriptor slot reclamation across completed, in-flight, ClearState, and OOM lists",
    "lanes-shader-cache-prepared": "A console shader translated before launch, loaded from the prepared cache",
    "lanes-shader-cache-compiled": "A console shader translated by the host worker while the game runs",
    "lanes-shader-cache-reused": "The worker's translation reused on the next launch without compiling",
    "lanes-package-replay": "Lanes gameplay replay (earlier guest with desktop graphics)", "cpu-replay": "8,388,608-move CPU replay",
}


def homebrew_matrix(evidence):
    rows = []
    for filename in sorted(evidence.glob("*.json")):
        data = json.loads(filename.read_text(encoding="utf-8"))
        if data.get("type") == "homebrew":
            rows.append((filename.name, data))

    gates = list(dict.fromkeys(r["gate"] for _, data in rows for r in data["checks"]))
    return rows, gates


def status():
    rows, gates = homebrew_matrix(ROOT / "evidence")
    lines = ["# Measured homebrew results", "",
             "Each column represents one run on its named host, provider and hardware. "
             "Each receipt records the guest output, exit status and binary hashes.", ""]
    names = [f"[{Path(name).stem}](../evidence/{name})" for name, _ in rows]
    pending_windows = not any(data["host"] == "Windows" for _, data in rows)
    if pending_windows:
        names.append("Physical Windows x64")
    lines.append("| Gate | " + " | ".join(names) + " |")
    lines.append("|---" * (1 + len(rows) + int(pending_windows)) + "|")
    for gate in gates:
        cells = []
        for _, d in rows:
            result = next((r for r in d["checks"] if r["gate"] == gate), None)
            cells.append("Pass" if result and result["passed"] else "Fail" if result else "—")
        if pending_windows:
            cells.append("Pending")
        lines.append("| " + " | ".join([GATE_LABELS.get(gate, gate), *cells]) + " |")
    lines += ["", "Rebuild this table from the repository root with "
              "`python tuxbox.py status > docs/RESULTS.md`. A dash means that the gate was omitted from that run. "
              "[Current Lanes replay](../examples/lanes/README.md#deterministic-replay) · [Xbox DLL checks](LIBRARIES.md#run-the-provider-checks) · [Retail results and priorities](PROGRESS.md)"]
    return "\n".join(lines)
