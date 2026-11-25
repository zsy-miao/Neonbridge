"""Keep Xbox constant-buffer offsets addressable on Linux D3D12 hosts."""
from pathlib import Path
import re
import subprocess

from .shader_prep import ROOT, matching_tuxbox_provider


def prepend(env, key, value, separator):
    current = env.get(key, "")
    # vkd3d accepts both commas and semicolons in its option lists.
    entries = re.split("[,;]", current) if separator == "," else current.split(separator)
    if value not in entries:
        env[key] = value + (separator + current if current else "")


def console_cbv_layer(env):
    layer = ROOT / "build/frosty-cbv-layer"
    sources = ROOT / "tools/reference/cbv-layer"
    build = ROOT / "tools/build_frosty_cbv_layer.sh"
    newest_source = max(path.stat().st_mtime_ns for path in (build, sources / "frosty.json", sources / "layer.c"))
    if any(not path.is_file() or path.stat().st_mtime_ns < newest_source
           for path in (layer / "frosty.json", layer / "frosty_cbv.so")):
        subprocess.run([build, layer], env=env, check=True)
    return str(layer)


def configure_console_cbv(backend, env, executable, report):
    if backend not in ("proton", "wine"):
        return
    if env.get("TUXBOX_REFERENCE_CBV") != "1":
        provider = executable.with_name("d3d12_xs.dll")
        if not provider.is_file():
            provider = next((path for path in executable.parent.iterdir() if path.name.lower() == "d3d12_xs.dll"), None)
        if provider is None or not matching_tuxbox_provider(provider):
            return

    feature = "cbv_16_byte_alignment"
    installation = env.get("PROTONPATH") if backend == "proton" else None
    marker = Path(installation) / "tuxbox-features" if installation else None
    if marker and marker.is_file() and feature in marker.read_text().splitlines():
        prepend(env, "VKD3D_CONFIG", feature, ",")
        # The fork binds 16-byte offsets as storage buffers on its descriptor-set path.
        prepend(env, "VKD3D_DISABLE_EXTENSIONS", "VK_EXT_descriptor_buffer", ",")
        report("Constant-buffer alignment: selected Proton build feature")
    else:
        # The layer hides VK_EXT_descriptor_buffer itself, only on GPUs that need it.
        layer = console_cbv_layer(env)
        path_key = "VK_LAYER_PATH" if env.get("VK_LAYER_PATH") else "VK_ADD_LAYER_PATH"
        prepend(env, path_key, layer, ":")
        prepend(env, "VK_INSTANCE_LAYERS", "VK_LAYER_TUXBOX_console_cbv", ":")
        if backend == "proton":
            prepend(env, "PRESSURE_VESSEL_FILESYSTEMS_RO", layer, ":")
        report("Constant-buffer alignment: TuxBox Vulkan layer")
