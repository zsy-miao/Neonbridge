"""Build Frosty's media bridges from the packaged codecs and a local Proton installation."""
import argparse
from pathlib import Path
import shutil
from tempfile import TemporaryDirectory

from build import Toolchain
from .host import Host
from .inspect import export_table


ROOT = Path(__file__).resolve().parents[1]
SOURCES = ROOT / "tools/reference"
LIBS = ("mfplat.lib", "mfuuid.lib", "d3d12.lib", "ole32.lib", "kernel32.lib")
BRIDGE = ("mfplat.dll", "mfplat_sys.dll")
AUDIO = ("frosty_packaged_aac.dll", "aac.exe")


def build_bridge(work, packaged_os, proton):
    work.mkdir(parents=True, exist_ok=True)
    desktop = work / "mfplat_sys.dll"
    shutil.copyfile(proton / "files/lib/wine/x86_64-windows/mfplat.dll", desktop)
    guest_slots, _ = export_table(packaged_os / "Windows/System32/mfplat.dll")
    host_slots, _ = export_table(desktop)
    guest = {name: ordinal for ordinal, names in guest_slots.items() for name in names}
    host = {name for names in host_slots.values() for name in names}
    reset = "MFResetDXGIDeviceManagerX"
    surface = "MFCreateDXGISurfaceBufferX"
    if reset not in guest or surface not in guest or "MFCreateDXGISurfaceBuffer" not in host:
        raise ValueError("The supplied Xbox or desktop MFPlat lacks the expected DXGI entry points")

    # Xbox code can import by ordinal. Leave private Xbox slots with no desktop
    # counterpart unassigned. Both packaged video codecs use the standard
    # surface-buffer signature for their Xbox entry point.
    rows = []
    for name, ordinal in sorted(guest.items(), key=lambda item: item[1]):
        if name == reset:
            rows.append(f"    {name} @{ordinal}")
        elif name == surface:
            rows.append(f"    {name}=mfplat_sys.MFCreateDXGISurfaceBuffer @{ordinal}")
        elif name in host:
            rows.append(f"    {name}=mfplat_sys.{name} @{ordinal}")

    # Wine's media components resolve additional named exports from app-local MFPlat.
    extra = sorted(host - guest.keys())
    rows.extend(f"    {name}=mfplat_sys.{name} @{i}" for i, name in enumerate(extra, max(guest_slots) + 1))
    definition = work / "mfplat.def"
    definition.write_text("LIBRARY mfplat\nEXPORTS\n" + "\n".join(rows) + "\n")
    tools = Toolchain()
    obj = tools.compile(SOURCES / "media/reset.cpp", work, "/W4", "/WX")
    tools.link(work / "mfplat.dll", obj, "d3d11.lib", "d3d12.lib", "dxgi.lib", "kernel32.lib", exports=definition)
    return BRIDGE


def build_video(work, packaged_os, codec):
    source = SOURCES / "hevc"
    original = {"hevc": "mfhevcdec.dll", "h264": "mfh264dec.dll"}[codec]
    packaged = original if codec == "hevc" else "frosty_packaged_h264.dll"
    shutil.copyfile(packaged_os / "Windows/System32" / original, work / packaged)
    adapter = f"frosty_{codec}.dll"
    register = f"register_{codec}.exe"
    definition = work / (codec + ".def")
    definition.write_text(f"LIBRARY frosty_{codec}\nEXPORTS\n    DllGetClassObject\n")
    tools = Toolchain()
    flags = ("/W4", "/WX", f"/I{ROOT / 'runtime/d3d12'}")
    transform = tools.compile(source / "transform.cpp", work, *flags)
    surface = tools.compile(source / "surface.cpp", work, *flags)
    tools.link(work / adapter, transform, surface, *LIBS, exports=definition)
    registration = tools.compile(source / "register.cpp", work, *flags)
    tools.link(work / register, registration, *LIBS, "advapi32.lib")
    return adapter, packaged, register


def build_audio(work, packaged_os):
    shutil.copyfile(packaged_os / "Windows/System32/MSAudDecMFT.dll", work / AUDIO[0])
    tools = Toolchain()
    obj = tools.compile(SOURCES / "aac/decode.cpp", work, "/W4", "/WX", f"/I{ROOT / 'runtime/d3d12'}")
    tools.link(work / AUDIO[1], obj, "mfplat.lib", "mfuuid.lib", "ole32.lib", "advapi32.lib", "kernel32.lib")
    return AUDIO


def build_frosty_media(destination, packaged_os, proton):
    """Install the Xbox MFPlat bridge, video adapters, packaged codecs and registration helpers."""
    destination.mkdir(parents=True, exist_ok=True)
    with TemporaryDirectory(dir=destination) as tmp:
        work = Path(tmp)
        files = (*build_bridge(work, packaged_os, proton), *build_video(work, packaged_os, "hevc"),
                 *build_video(work, packaged_os, "h264"), *build_audio(work, packaged_os))
        for name in files:
            (work / name).replace(destination / name)
    return files


def register_frosty_media(destination, proton, prefix):
    """Register the staged codecs at their final paths. The decoders also require TuxBox XMem."""
    destination = destination.resolve()
    host = Host("proton", proton, prefix)
    host.env["WINEDLLOVERRIDES"] = ";".join(filter(None, [host.env.get("WINEDLLOVERRIDES"), "mfplat=n,b"]))
    host.env.setdefault("UMU_RUNTIME_UPDATE", "0")
    with TemporaryDirectory(dir=destination) as tmp:
        receipt = Path(tmp).relative_to(destination) / "aac.txt"
        for program, args in (("register_hevc.exe", ["frosty_hevc.dll"]),
                              ("register_h264.exe", ["frosty_h264.dll", "h264"]),
                              ("aac.exe", ["register", str(receipt)])):
            result = host.run(destination / program, args, capture=True, timeout=65)
            if result.returncode:
                details = (destination / receipt).read_text() if (destination / receipt).exists() else ""
                raise RuntimeError(f"{program} returned {result.returncode}\n{details}{result.stdout.decode(errors='replace')[-1200:]}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("destination", "packaged-os", "proton"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--prefix", type=Path, help="also register the staged codecs in this Proton prefix")
    args = parser.parse_args()
    build_frosty_media(args.destination, args.packaged_os, args.proton)
    if args.prefix:
        register_frosty_media(args.destination, args.proton, args.prefix)


if __name__ == "__main__":
    main()
