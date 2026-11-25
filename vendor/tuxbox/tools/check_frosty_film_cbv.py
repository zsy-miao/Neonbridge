#!/usr/bin/env python3
"""Read all 48 registers from the film shader's CBV at every 16-byte offset within the desktop alignment."""
import argparse
import json
import os
from pathlib import Path
import struct
import subprocess
import sys
from tempfile import TemporaryDirectory

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from build import find_dxc
from frosty_gpu_guest import run_gpu_guest
from project_tuxbox.inspect import digest


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("provider", "proton", "prefix", "compiler-dir", "work", "output"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--cbv-layer", type=Path, help="Directory containing the built Frosty CBV layer")
    parser.add_argument("--vkd3d-dir", type=Path, help="Test a build of d3d12.dll and d3d12core.dll with this Proton installation")
    args = parser.parse_args()
    layer = None
    if args.cbv_layer:
        directory = args.cbv_layer.resolve()
        manifest = json.loads((directory / "frosty.json").read_text())["layer"]
        library = directory / manifest["library_path"]
        # Explicit VK_LAYER_PATH takes priority over VK_ADD_LAYER_PATH.
        path_var = "VK_LAYER_PATH" if os.environ.get("VK_LAYER_PATH") else "VK_ADD_LAYER_PATH"
        for key, value, separator in ((path_var, str(directory), os.pathsep),
                                      ("VK_INSTANCE_LAYERS", manifest["name"], os.pathsep),
                                      ("VKD3D_DISABLE_EXTENSIONS", "VK_EXT_descriptor_buffer", ";")):
            settings = [item for item in os.getenv(key, "").split(separator) if item]
            if value not in settings:
                settings.insert(0, value)
            os.environ[key] = separator.join(settings)
        layer = dict(name=manifest["name"], library_sha256=digest(library),
                     disabled_extensions=os.environ["VKD3D_DISABLE_EXTENSIONS"],
                     vkd3d_config=os.getenv("VKD3D_CONFIG", ""))
    work, proton = args.work.resolve(), args.proton.resolve()
    work.mkdir(parents=True, exist_ok=True)
    sources = ROOT / "tools/reference/film-cbv"
    with TemporaryDirectory(dir=work) as tmp:
        trial = Path(tmp)
        subprocess.run([find_dxc(None), "-T", "cs_6_0", "-E", "main", "-Fo", trial / "film-cbv.cso",
                        sources / "work.hlsl"], check=True)
        exe, status, actual = run_gpu_guest(sources / "guest.cpp", "film-cbv.exe", trial, work, proton, args.prefix,
                                            args.provider, args.compiler_dir, vkd3d_dir=args.vkd3d_dir)
        fields = ",".join(f"{v:08x}" for v in struct.unpack("<4I", struct.pack("<4f", 0, 0, 1/2976, 1/1240)))
        matched = all(actual.get(f"offset_{offset}_words") == "192" and actual.get(f"offset_{offset}_fields") == fields
                      for offset in range(0, 256, 16))
        adapters = [line.split("Found device: ", 1)[1] for line in (work / "gpu.log").read_text().splitlines()
                    if "Found device: " in line]
        receipt = dict(passed=not status and matched, guest_exit_code=status,
                       provider_sha256=digest(args.provider), guest_sha256=digest(exe),
                       adapters=adapters, proton=(proton / "version").read_text().strip(), retail_gpuva="0x1d5235c0",
                       size_bytes=768, retail_b0_fields=["cb0[37].x", "cb0[37].y", "cb0[38].z", "cb0[38].w"],
                       expected_ieee754=fields, scope="Guest CBV creation, guest descriptor range copy, shader-visible heap fetch, post-recording upload update, all 48 registers read by a compute control.",
                       readback=actual)
        if layer:
            receipt["layer"] = layer
        receipt["vkd3d"] = {
            "dll_sha256": {lib: digest(trial / lib) for lib in ("d3d12.dll", "d3d12core.dll")},
            "config": os.getenv("VKD3D_CONFIG", ""),
            "disabled_extensions": os.getenv("VKD3D_DISABLE_EXTENSIONS", ""),
        }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(receipt, indent=2) + "\n")
    if not receipt["passed"]:
        raise ValueError(f"GPU control mismatch: {actual}")


if __name__ == "__main__":
    main()
