"""Private, bounded snapshots for same-input compiler comparisons."""
from pathlib import Path
import re
import shutil
import stat

from project_tuxbox.inspect import digest
from project_tuxbox.runtime_shaders import REQUEST_PATTERNS, verified_input, portable_pixel


def snapshot_request(source, directory):
    """Copy a digest-verified request, never a hardlink to mutable inputs."""
    if source.suffix not in REQUEST_PATTERNS or not re.fullmatch(
            REQUEST_PATTERNS[source.suffix], source.stem):
        raise ValueError("invalid request key")
    dest = directory / source.name
    if source.suffix == ".link":
        if not stat.S_ISDIR(source.lstat().st_mode):
            raise ValueError("linked request must be a real directory")
        dest.mkdir()
        for name, expected in zip(("producer.dxbc", "pixel.dxbc", "pixel.cso"), source.stem.split("_")):
            data = verified_input(source / name, expected)
            if name == "pixel.cso" and not portable_pixel(data):
                raise ValueError("pixel order must be portable pixel DXIL")
            (dest / name).write_bytes(data)
    else:
        expected = source.stem.split("_rt")[0]
        dest.write_bytes(verified_input(source, expected))
    return dest


def pin_emitter(source, directory):
    source = Path(source).resolve()
    dest = directory / "xshader"
    directory.mkdir()
    before = digest(source)
    shutil.copyfile(source, dest)
    dest.chmod(0o700)
    if before != digest(dest) or before != digest(source):
        raise ValueError("emitter changed during snapshot")
    return dest, before
