#!/usr/bin/env python3
"""Register the packaged AAC decoder in a dedicated prefix and verify the 5.1 intro soundtrack."""
import argparse
from array import array
import hashlib
import json
import math
from pathlib import Path
import shutil
import struct
import subprocess
import sys
from tempfile import TemporaryDirectory

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from project_tuxbox.media import build_audio
from check_frosty_hevc import run_host
from project_tuxbox.host import Host
from project_tuxbox.inspect import digest


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("packaged-os", "video", "hevc-work", "proton", "prefix", "work", "output"):
        parser.add_argument("--" + name, type=Path, required=True)
    args = parser.parse_args()
    work = args.work.resolve()
    work.mkdir(exist_ok=True, parents=True)
    dll = work / "frosty_packaged_aac.dll"
    guest = work / "aac.exe"
    build_audio(work, args.packaged_os)
    for name in ("xmem.dll", "mfplat.dll", "mfplat_sys.dll"):
        shutil.copyfile(args.hevc_work / name, work / name)
    host = Host("proton", args.proton, args.prefix)
    run_host(host, guest, work, "register", "registration.txt")
    packets = json.loads(subprocess.check_output(["ffprobe", "-v", "error", "-select_streams", "a:0",
        "-show_packets", "-show_entries", "packet=size,pos", "-of", "json", str(args.video)]))["packets"]
    adts = bytearray()
    with TemporaryDirectory(dir=work) as temp:
        trial = Path(temp)
        with args.video.open("rb") as movie, (trial / "access-units.bin").open("wb") as guest_input:
            for packet in packets[:96]:
                n = int(packet["size"])
                movie.seek(int(packet["pos"]))
                raw = movie.read(n)
                if n != len(raw) or n + 7 > 8191:
                    raise ValueError("Unexpected AAC access unit")
                guest_input.write(struct.pack("<I", n) + raw)
                # ADTS LC, 48 kHz, six channels. FFmpeg takes the identical raw AAC
                # access units through ADTS without applying MP4 edit-list priming.
                size = n + 7
                adts.extend([0xff, 0xf1, 0x4d, 0x80 | (size >> 11), (size >> 3) & 255,
                             ((size & 7) << 5) | 31, 0xfc])
                adts.extend(raw)
        run_host(host, guest, trial, "decode", "result.txt")
        actual = (trial / "audio.pcm").read_bytes()
        oracle = subprocess.check_output(["ffmpeg", "-v", "error", "-f", "aac", "-i", "pipe:0",
                                          "-map", "0:a:0", "-f", "f32le", "pipe:1"], input=adts)
        result = dict(line.split("=", 1) for line in (trial / "result.txt").read_text().splitlines())
    if result.get("float") != "1" or len(oracle) != len(actual) or len(actual) != 96*1024*6*4:
        raise ValueError(f"Unexpected PCM dimensions: {result}")
    a, b = array("f"), array("f")
    a.frombytes(actual)
    b.frombytes(oracle)
    max_error = max(abs(x-y) for x, y in zip(a, b))
    similarity = []
    for i in range(6):
        got, expected = a[i::6], b[i::6]
        dot = sum(x*y for x, y in zip(got, expected))
        similarity.append(dot / math.sqrt(sum(x*x for x in got) * sum(y*y for y in expected)))
    if not max_error < 0.0001 or min(similarity) < 0.99999:
        raise ValueError(f"PCM differs from FFmpeg: max error {max_error}, channel similarities {similarity}")
    result.update(host=host.version, source_sha256=digest(args.video), decoder_sha256=digest(dll),
                  guest_sha256=digest(guest), pcm_sha256=hashlib.sha256(actual).hexdigest(),
                  ffmpeg_sha256=hashlib.sha256(oracle).hexdigest(), pcm_max_absolute_error=max_error,
                  channel_cosine_similarity=similarity)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2) + "\n")


if __name__ == "__main__":
    main()
