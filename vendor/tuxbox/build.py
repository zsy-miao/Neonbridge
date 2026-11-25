"""Build the Project TuxBox x64 guests and their app-local DLLs."""
import argparse
import functools
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile


ROOT = Path(__file__).resolve().parent
OUTPUT = ROOT / "build"
TARGETS = ("bvh", "route", "graphics", "xmem", "acphal", "xapu", "xgameruntime", "xcrdapi", "dxc-utils", "xg", "gameinput",
           "d3d12", "lanes")
# rustc --print native-static-libs for the Windows MSVC target with +crt-static.
RUST_STD_LIBS = ("kernel32.lib", "ntdll.lib", "userenv.lib", "ws2_32.lib", "dbghelp.lib", "synchronization.lib",
                 "/defaultlib:libcmt")


def xwin_root():
    cache = Path(os.environ.get("XDG_CACHE_HOME", Path.home() / ".cache"))
    return Path(os.environ.get("XWIN_SDK_ROOT", cache / "cargo-xwin/xwin")).expanduser().resolve()


def require_tool(name):
    executable = shutil.which(str(name))
    if executable is None:
        raise ValueError(f"Could not find executable {name!s}")
    return str(Path(executable).absolute())


class Toolchain:
    def __init__(self):
        self.includes = []
        self.libraries = []
        if os.name == "nt":
            if os.environ.get("VSCMD_ARG_TGT_ARCH", "x64").lower() != "x64":
                raise ValueError("Open an x64 Native Tools Command Prompt for Visual Studio")
            self.compiler = ["cl"]
            self.linker = require_tool("link")
        else:
            sdk = xwin_root()
            if not (sdk / "sdk/include/um/windows.h").is_file():
                raise ValueError(f"Set XWIN_SDK_ROOT to the xwin SDK/CRT tree. Looked in {sdk}")
            self.compiler = ["clang-cl", "--target=x86_64-pc-windows-msvc"]
            self.linker = require_tool("lld-link")
            for part in ("crt/include", "sdk/include/ucrt", "sdk/include/shared",
                         "sdk/include/um", "sdk/include/winrt"):
                self.includes.extend(["-imsvc", str(sdk / part)])
            self.libraries = [f"/libpath:{sdk / part}" for part in
                              ("crt/lib/x86_64", "sdk/lib/ucrt/x86_64", "sdk/lib/um/x86_64")]

    def compile(self, source, directory, *extra):
        language = ["/std:c11"] if source.suffix == ".c" else ["/std:c++17", "/EHsc"]
        obj = source.stem + ".obj"
        directory.mkdir(parents=True, exist_ok=True)
        subprocess.run([require_tool(self.compiler[0]), *self.compiler[1:], "/nologo", "/c", "/O2", "/MT", "/Brepro", "/utf-8",
                        *language, *self.includes, *extra, *(["-Wno-cast-function-type"] if self.compiler[0] == "clang-cl" else []), f"/Fo{obj}", str(source)],
                       cwd=directory, check=True)
        return obj

    def rust_library(self, package, *, target_dir=None):
        target = "x86_64-pc-windows-msvc"
        directory = (target_dir or OUTPUT / "cargo").resolve()
        subprocess.run([require_tool("cargo"), "build", "--locked", "--release", "--target",
                        target, "--target-dir", str(directory), "--package", package],
                       cwd=ROOT, check=True)
        return directory / target / "release" / (package.replace("-", "_") + ".lib")

    def link(self, output, *inputs, exports=None):
        flags = ["/subsystem:console"]
        if exports is not None:
            flags = ["/dll", f"/def:{exports}", f"/implib:{output.stem}.lib"]
        subprocess.run([self.linker, "/nologo", "/machine:x64", "/Brepro", "/incremental:no",
                        *flags, f"/out:{output.name}", *self.libraries, *inputs],
                       cwd=output.parent, check=True)
        print(output, flush=True)


DXC_HELP = ("Use a Microsoft DXC release: set DXC (or pass --dxc PATH to build.py). "
            "https://github.com/microsoft/DirectXShaderCompiler/releases")


@functools.cache
def dxc_probe_error(dxc):
    """Compile a trivial shader once per compiler path; return why it failed, or None."""
    # Some distribution builds abort on every shader, so try a trivial one before the guest build needs it.
    with tempfile.TemporaryDirectory(prefix="tuxbox-dxc-") as tmp:
        source = Path(tmp) / "probe.hlsl"
        source.write_text("RWStructuredBuffer<uint> r : register(u0);\n"
                          "[numthreads(1, 1, 1)] void main(uint3 i : SV_DispatchThreadID) { r[i.x] = i.x; }\n")
        try:
            run = subprocess.run([dxc, "-T", "cs_6_0", "-E", "main", "-Fo", str(source.with_suffix(".cso")),
                                  str(source)], capture_output=True, text=True, timeout=30)
        except subprocess.TimeoutExpired:
            return f"{dxc} did not compile a minimal compute shader within 30 seconds"
        except OSError as error:
            return f"{dxc} could not be started ({error})"
    if run.returncode != 0:
        detail = (run.stderr or run.stdout).strip().splitlines()
        return (f"{dxc} could not compile a minimal compute shader (exit {run.returncode}"
                f"{': ' + detail[0] if detail else ''})")
    return None


def check_dxc(dxc):
    error = dxc_probe_error(dxc)
    if error:
        raise ValueError(f"{error}. {DXC_HELP}")
    return dxc


def find_dxc(override):
    if override or os.environ.get("DXC"):
        requested = Path(override or os.environ["DXC"]).expanduser()
        return check_dxc(require_tool(requested.resolve() if requested.is_file() else requested))
    # Without an explicit choice, skip a broken compiler on PATH in favour of the staged toolchain.
    candidates = [shutil.which("dxc")]
    local = OUTPUT / "d3d12-smoke/toolchain/bin" / ("dxc.exe" if os.name == "nt" else "dxc")
    if local.is_file():
        candidates.append(require_tool(local))
    failures = []
    for dxc in filter(None, dict.fromkeys(candidates)):
        error = dxc_probe_error(dxc)
        if error is None:
            return dxc
        failures.append(error)
    if failures:
        raise ValueError("; ".join(failures) + f". {DXC_HELP}")
    raise ValueError("Install DirectX Shader Compiler (dxc), set DXC, or pass --dxc PATH")


def d3d12_exports(src, out, reference):
    """Select Xbox ordinals independently of the in-process device creation profile."""
    definition = src / "d3d12_xs.def"
    lines = definition.read_text().splitlines()
    if reference is not None:
        import pefile
        from project_tuxbox.inspect import export_table
        try:
            slots, data = export_table(reference)
        except pefile.PEFormatError as error:
            raise ValueError(f"{reference.name}: {error}") from error
        ordinals = {name: ordinal for ordinal, names in slots.items() for name in names}
    entries, identifiers, functions = [], [], []
    for line in lines[2:]:
        symbol, ordinal, *flags = line.split()
        name = symbol.partition("=")[0]
        if reference is not None:
            slot = ordinals.get(name)
            if slot is None:
                # The frame ABI guest imports the device IID. Additional SDK IDs
                # vary with the GDK build. D3D12ZeroMemoryX is Series-specific.
                if (flags == ["DATA"] and name != "IID_ID3D12Device") or name in {
                        "D3D12ZeroMemoryX", "GetFrameBufferPreserveMemorySizeBytes", "D3D12ConfigureVirtualMemoryX"}:
                    continue
                raise ValueError(f"{reference.name} lacks {name}, required by the D3D12 guests")
            if ("DATA" in flags) != (slot in data):
                raise ValueError(f"{reference.name} has an incompatible export type for {name}")
            ordinal = f"@{slot}"
        entries.append("    " + " ".join([symbol, ordinal, *flags]))
        if "DATA" in flags:
            expected = f"__uuidof({name[4:]})" if name.startswith("IID_") else name
            identifiers.append(f"GUID_EXPORT({name}, {expected}, {ordinal[1:]})")
        else:
            functions.append(f"#define ORD_{name} {ordinal[1:]}")
    (out / "function-ordinals.inc").write_text("\n".join(functions) + "\n")
    (out / "guid-exports.inc").write_text("\n".join(identifiers) + "\n")
    if reference is not None:
        definition = out / "d3d12-reference.def"
        definition.write_text("\n".join(lines[:2] + entries) + "\n")
    return definition


def build_target(tools, target, dxc, d3d12_reference=None):
    if target == "route":
        obj = tools.compile(ROOT / "fixtures/route_game.c", OUTPUT, "/W4", "/WX")
        tools.link(OUTPUT / "route-game.exe", obj, "kernel32.lib")
    elif target == "graphics":
        out = OUTPUT / "graphics"
        obj = tools.compile(ROOT / "graphics/desktop_clear.cpp", out)
        tools.link(out / "desktop-clear.exe", obj, "dxgi.lib", "d3d12.lib", "kernel32.lib")
    elif target == "xmem":
        src, out = ROOT / "runtime/xmem", OUTPUT / "xmem-smoke"
        out.mkdir(parents=True, exist_ok=True)
        # Link tuxbox_xhttp first so xmem.def's forwarder resolves under MSVC link.exe.
        tools.link(out / "tuxbox_xhttp.dll", tools.rust_library("tuxbox-xmem-http"), "winhttp.lib", *RUST_STD_LIBS, exports=src / "xhttp.def")
        dll = tools.rust_library("tuxbox-xmem")
        # Force DllMain from tuxbox_xmem before libcmt's stub to avoid LNK2005 on MSVC.
        tools.link(out / "xmem.dll", dll, "tuxbox_xhttp.lib", "/include:DllMain", *RUST_STD_LIBS, exports=src / "xmem.def")
        http = tools.compile(src / "http_control.cpp", out, "/W4", "/WX")
        tools.link(out / "xmem-http.exe", http, "xmem.lib", "tuxbox_xhttp.lib", "kernel32.lib")
        guest = tools.compile(src / "guest.c", out, "/W4")
        tools.link(out / "xmem-smoke.exe", guest, "xmem.lib", "kernel32.lib")
        partitions = tools.compile(src / "partition_control.c", out, "/W4", "/WX")
        tools.link(out / "xmem-partitions.exe", partitions, "xmem.lib", "kernel32.lib")
    elif target in ("xgameruntime", "xcrdapi"):
        src, out = ROOT / "runtime" / target, OUTPUT / target
        out.mkdir(parents=True, exist_ok=True)
        archive = tools.rust_library(f"tuxbox-{target}")
        tools.link(out / f"{target}.dll", archive, *RUST_STD_LIBS, exports=src / f"{target}.def")
        guest = tools.compile(src / "guest.cpp", out, "/W4", "/WX")
        binary = "runtime-control.exe" if target == "xgameruntime" else "xcrdapi-read.exe"
        tools.link(out / binary, guest, f"{target}.lib", "kernel32.lib")
    elif target == "bvh":
        out = OUTPUT / "bvh"
        out.mkdir(parents=True, exist_ok=True)
        tools.link(out / "XboxBvhBuilder.dll", tools.rust_library("tuxbox-bvh"),
                   *RUST_STD_LIBS, exports=ROOT / "runtime/bvh/XboxBvhBuilder.def")
    elif target == "xapu":
        out = OUTPUT / "xapu"
        out.mkdir(parents=True, exist_ok=True)
        archive = tools.rust_library("tuxbox-xapu")
        for provider in ("xapu", "chad"):
            tools.link(out / f"{provider}.dll", archive, *RUST_STD_LIBS,
                       exports=ROOT / "runtime/xapu" / f"{provider}.def")
        control = tools.compile(ROOT / "runtime/xapu/guest.c", out, "/W4", "/WX")
        tools.link(out / "xapu-control.exe", control, "xapu.lib", "kernel32.lib")
    elif target == "acphal":
        src, out = ROOT / "runtime/acphal", OUTPUT / "acphal"
        out.mkdir(parents=True, exist_ok=True)
        archive = tools.rust_library("tuxbox-acphal")
        tools.link(out / "AcpHal.dll", archive, *RUST_STD_LIBS, exports=src / "AcpHal.def")
    elif target == "xg":
        src, out = ROOT / "runtime/xg", OUTPUT / "xg-provider"
        out.mkdir(parents=True, exist_ok=True)
        tools.link(out / "xg_xs.dll", tools.rust_library("tuxbox-xg"), *RUST_STD_LIBS,
                   exports=src / "xg_xs.def")
        library = tools.compile(src / "library_control.cpp", out, "/W4", "/WX")
        tools.link(out / "xg-library.exe", library, "xg_xs.lib", "kernel32.lib")
        dcc = tools.compile(src / "dcc_control.cpp", out, "/W4", "/WX")
        tools.link(out / "xg-dcc.exe", dcc, "xg_xs.lib", "kernel32.lib")
        guest = tools.compile(src / "guest.cpp", out, "/W4", "/WX")
        tools.link(out / "xg-texture.exe", guest, "xg_xs.lib", "kernel32.lib")
        # The optional console control uses the same guest and its own supplied DLL.
        control = OUTPUT / "xg"
        control.mkdir(exist_ok=True)
        shutil.copyfile(out / "xg-texture.exe", control / "xg-texture.exe")
    elif target == "dxc-utils":
        src, out = ROOT / "runtime/dxc", OUTPUT / "dxc-utils"
        out.mkdir(parents=True, exist_ok=True)
        tools.link(out / "dxcompiler_xs.dll", tools.rust_library("tuxbox-dxc"), *RUST_STD_LIBS,
                   exports=src / "dxcompiler_xs.def")
        guest = tools.compile(src / "guest.cpp", out, "/W4", "/WX")
        tools.link(out / "dxc-utils.exe", guest, "dxcompiler_xs.lib", "kernel32.lib")
        xdxr = tools.compile(src / "xdxr_guest.cpp", out, "/W4", "/WX")
        tools.link(out / "xdxr-link.exe", xdxr, "dxcompiler_xs.lib", "kernel32.lib", "ole32.lib")
    elif target == "d3d12":
        src, out = ROOT / "runtime/d3d12", OUTPUT / "d3d12-smoke"
        out.mkdir(parents=True, exist_ok=True)
        provider = tools.rust_library("tuxbox-d3d12")
        tools.link(out / "d3d12_xs.dll", provider, "dxgi.lib", "d3d12.lib", "d3dcompiler.lib", "user32.lib",
                   *RUST_STD_LIBS, exports=d3d12_exports(src, out, d3d12_reference))
        signature = tools.compile(src / "signature_guest.cpp", out, "/W4", "/WX")
        tools.link(out / "d3d12-signatures.exe", signature, "d3d12_xs.lib", "kernel32.lib")
        ids = tools.compile(src / "guid_guest.cpp", out, "/W4", "/WX", f"/I{out}")
        tools.link(out / "d3d12-guids.exe", ids, "d3d12_xs.lib", "dxguid.lib", "kernel32.lib")
        exports = tools.compile(src / "exports_guest.cpp", out, "/W4", "/WX", f"/I{out}")
        tools.link(out / "d3d12-exports.exe", exports, "d3d12_xs.lib", "kernel32.lib")
        for name, source in (("features", "feature_guest.cpp"), ("queries", "query_guest.cpp"),
                             ("queues", "queue_guest.cpp"), ("heaps", "heap_guest.cpp"),
                             ("placement", "placement_guest.cpp"), ("buffer-import", "buffer_import_guest.cpp"), ("copy-resource", "copy_resource_guest.cpp"),
                             ("frame-events", "frame_event_guest.cpp")):
            obj = tools.compile(src / source, out, "/W4", "/WX")
            tools.link(out / f"d3d12-{name}.exe", obj, "d3d12_xs.lib", "kernel32.lib", "dxgi.lib")
        factory = tools.compile(src / "factory_guest.cpp", out, "/W4", "/WX")
        tools.link(out / "d3d12-factory.exe", factory, "d3d12_xs.lib", "kernel32.lib", "dxgi.lib")
        frame = tools.compile(src / "frame_guest.cpp", out, "/W4", "/WX")
        display = tools.compile(src / "frame_present.cpp", out, "/W4", "/WX")
        tools.link(out / "xbox-frame.exe", frame, display, "d3d12_xs.lib", "dxgi.lib",
                   "user32.lib", "gdi32.lib", "kernel32.lib")
        rgb10 = tools.compile(src / "frame_guest.cpp", out, "/W4", "/WX", "/DGUEST_FRAME_RGB10")
        tools.link(out / "xbox-frame-rgb10.exe", rgb10, display, "d3d12_xs.lib", "dxgi.lib",
                   "user32.lib", "gdi32.lib", "kernel32.lib")
        size = tools.compile(src / "frame_guest.cpp", out, "/W4", "/WX", "/DGUEST_FRAME_RGB10", "/DGUEST_FRAME_4K")
        tools.link(out / "xbox-frame-4k.exe", size, display, "d3d12_xs.lib", "dxgi.lib",
                   "user32.lib", "gdi32.lib", "kernel32.lib")
        negative = tools.compile(src / "frame_negative.cpp", out, "/W4", "/WX")
        tools.link(out / "xbox-frame-negative.exe", negative, "d3d12_xs.lib", "dxgi.lib", "kernel32.lib")
        for shader, entry, profile, header, variable in (
            ("affine", "main", "cs_6_0", "affine", "affine_dxil"),
            ("affine", "accumulate", "cs_6_0", "affine_accumulate", "accumulate_dxil"),
            ("table", "main", "cs_6_0", "table", "table_dxil"),
            ("root_cbv", "main", "cs_6_0", "root_cbv", "root_cbv_dxil"),
            ("triangle", "VSMain", "vs_6_0", "triangle_vs", "triangle_vs"),
            ("triangle", "VSFromBuffer", "vs_6_0", "triangle_input_vs", "triangle_input_vs"),
            ("triangle", "PSMain", "ps_6_0", "triangle_ps", "triangle_ps"),
            ("rectangle", "VSMain", "vs_6_0", "rectangle_vs", "rectangle_vs"),
            ("rectangle", "PSMain", "ps_6_0", "rectangle_ps", "rectangle_ps"),
            ("mesh_groups", "mesh", "ms_6_5", "mesh_ms", "mesh_ms"),
            ("mesh_groups", "mesh", "ms_6_5", "mesh_ms_payload", "mesh_ms_payload"),
            ("mesh_groups", "amplify", "as_6_5", "mesh_as", "mesh_as"),
            ("mesh_groups", "pixel", "ps_6_5", "mesh_ps", "mesh_ps"),
        ):
            payload = ["-DWITH_PAYLOAD=1"] if header == "mesh_ms_payload" else []
            subprocess.run([dxc, "-T", profile, "-E", entry, "-O3", *payload, "-Fh", f"{header}.h",
                            "-Vn", variable, str(src / f"{shader}.hlsl")], cwd=out, check=True)
        for name, source in (("tables", "table_guest.cpp"), ("indirect", "indirect_guest.cpp"),
                             ("draw", "graphics_guest.cpp"), ("mesh", "mesh_guest.cpp"),
                             ("null-usage", "null_usage_guest.cpp"), ("root-cbv-window", "root_cbv_guest.cpp")):
            obj = tools.compile(src / source, out, "/W4", "/WX", f"/I{out}")
            tools.link(out / f"d3d12-{name}.exe", obj, "d3d12_xs.lib", "dxgi.lib", "kernel32.lib")
        guest = tools.compile(src / "guest.cpp", out, f"/I{out}")
        # These mixed-ABI guests serialize Xbox signatures on a desktop device.
        subprocess.run([tools.linker, "/lib", "/nologo", "/machine:x64",
                        f"/def:{src / 'signature-imports.def'}", "/out:guest-signatures.lib"], cwd=out, check=True)
        tools.link(out / "d3d12-root-smoke.exe", guest, "guest-signatures.lib", "d3d12.lib",
                   "dxgi.lib", "kernel32.lib")
    elif target == "gameinput":
        src, out = ROOT / "runtime/gameinput", OUTPUT / "gameinput"
        archive = tools.rust_library("tuxbox-gameinput")
        abi = tools.compile(src / "abi.cpp", out, "/W4", "/WX")
        tools.link(out / "GameInput.dll", abi, archive, "user32.lib", "xinput.lib", *RUST_STD_LIBS,
                   exports=src / "GameInput.def")
        subprocess.run([tools.linker, "/lib", "/nologo", "/machine:x64", f"/def:{src / 'guest-imports.def'}",
                        "/out:guest-gameinput.lib"], cwd=out, check=True)
        guest = tools.compile(src / "guest.cpp", out, "/W4", "/WX")
        tools.link(out / "gameinput-control.exe", guest, "guest-gameinput.lib", "user32.lib", "kernel32.lib")
    elif target == "lanes":
        src = ROOT / "examples/lanes"
        objects, package = OUTPUT / "lanes-obj", OUTPUT / "lanes"
        objects.mkdir(parents=True, exist_ok=True)
        package.mkdir(parents=True, exist_ok=True)
        subprocess.run([dxc, "-T", "cs_6_0", "-E", "main", "-O3", "-Fh", "course_shader.h",
                        "-Vn", "course_dxil", str(src / "course.hlsl")], cwd=objects, check=True)
        course = tools.compile(src / "course.cpp", objects, f"/I{objects}")
        screen = tools.compile(src / "screen.cpp", objects)
        controls = tools.compile(src / "input.cpp", objects)
        game = tools.compile(src / "game.cpp", objects)
        subprocess.run([sys.executable, str(src / "shader_fixture.py"), "xbox_shader.h"], cwd=objects, check=True)
        selftest = tools.compile(src / "selftest.cpp", objects, f"/I{objects}")
        xmem, d3d, gameinput = OUTPUT / "xmem-smoke", OUTPUT / "d3d12-smoke", OUTPUT / "gameinput"
        tools.link(objects / "lanes.exe", course, screen, controls, game, selftest, xmem / "xmem.lib",
                   d3d / "d3d12_xs.lib",
                   gameinput / "guest-gameinput.lib", "onecore_apiset.lib", "/nodefaultlib:kernel32.lib")
        for source in (objects / "lanes.exe", xmem / "xmem.dll", xmem / "tuxbox_xhttp.dll", d3d / "d3d12_xs.dll",
                       OUTPUT / "bvh/XboxBvhBuilder.dll", gameinput / "GameInput.dll", src / "MicrosoftGame.config"):
            shutil.copyfile(source, package / source.name)
        print(package / "MicrosoftGame.config", flush=True)



def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("target", nargs="?", default="all", choices=("all", *TARGETS))
    parser.add_argument("--dxc", metavar="PATH", help="DXC executable to compile the D3D12 guest shader")
    parser.add_argument("--d3d12-reference", type=Path, metavar="DLL",
                        help="use export ordinals from this supplied x64 D3D12 Xbox DLL")
    args = parser.parse_args()
    try:
        # Preserve the standalone MinGW route build on hosts without an xwin SDK.
        if (args.target == "route" and os.name != "nt"
                and (not (xwin_root() / "sdk/include/um/windows.h").is_file()
                     or not shutil.which("clang-cl") or not shutil.which("lld-link"))
                and (mingw := shutil.which("x86_64-w64-mingw32-gcc"))):
            OUTPUT.mkdir(parents=True, exist_ok=True)
            subprocess.run([mingw, "-std=c11", "-O2", "-Wall", "-Wextra", "-Werror", "-static",
                            "-municode", "-Wl,--no-insert-timestamp", str(ROOT / "fixtures/route_game.c"),
                            "-o", "route-game.exe"], cwd=OUTPUT, check=True)
            print(OUTPUT / "route-game.exe")
            return 0
        targets = TARGETS if args.target == "all" else (args.target,)
        if args.target == "xg":
            targets = ("xmem", "xg")
        if args.target == "lanes":
            targets = ("xmem", "d3d12", "gameinput", "lanes")
        dxc = find_dxc(args.dxc) if "d3d12" in targets else None
        tools = Toolchain()
        for target in targets:
            build_target(tools, target, dxc, args.d3d12_reference)
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        parser.exit(1, f"Build failed: {error}\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
