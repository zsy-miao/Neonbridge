"""Build the Lanes package using the project toolchain."""
from pathlib import Path
import subprocess
import sys


if __name__ == "__main__":
    builder = Path(__file__).resolve().parents[2] / "build.py"
    sys.exit(subprocess.call([sys.executable, str(builder), "lanes", *sys.argv[1:]]))
