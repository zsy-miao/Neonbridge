"""Emplacements des DLL console de TomBox (sous-module third_party/tombox).

`tools/build_tombox.py` compile les DLL et les copie dans TOMBOX_INSTALL, dossier par défaut du
runtime. ANYXBOXX_TOMBOX_DIR le remplace. Le lanceur C++ (runtime/launcher) applique la même règle
avec `..\tombox` à côté de son dossier.
"""
from pathlib import Path

REPOSITORY = Path(__file__).resolve().parents[2]
TOMBOX_SOURCE = REPOSITORY / "third_party" / "tombox"
TOMBOX_CARGO_OUTPUT = TOMBOX_SOURCE / "target" / "release"
TOMBOX_INSTALL = REPOSITORY / "runtime" / "build" / "tombox"
TOMBOX_ENV = "ANYXBOXX_TOMBOX_DIR"
# DLL de TomBox servies à la place des nôtres : TomBox exporte davantage (inventaire dans
# docs/research/first-boot-1021.md). xgameruntime, kernelx et anyxboxx_tls restent les nôtres.
TOMBOX_PREFERRED = ("d3d12_xs.dll", "xmem.dll", "xcrdapi.dll")
BUILD_COMMAND = "python tools/build_tombox.py"
