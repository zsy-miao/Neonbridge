"""Compatibility entry point for the graphics fixture build."""
from pathlib import Path
import subprocess
import sys


if __name__ == "__main__":
    builder = Path(__file__).resolve().parents[1] / "build.py"
    sys.exit(subprocess.call([sys.executable, str(builder), "graphics", *sys.argv[1:]]))
