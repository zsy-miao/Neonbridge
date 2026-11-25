"""Compatibility entry point for the d3d12 fixture build."""
from pathlib import Path
import subprocess
import sys


if __name__ == "__main__":
    builder = Path(__file__).resolve().parents[2] / "build.py"
    sys.exit(subprocess.call([sys.executable, str(builder), "d3d12", *sys.argv[1:]]))
