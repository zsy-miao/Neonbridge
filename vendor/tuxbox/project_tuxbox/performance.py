"""Summarize the per-present frame times recorded by MangoHud on Linux."""
import csv
import json
import math
from pathlib import Path
import re
from tempfile import TemporaryDirectory


def read_frame_times(path, run_seconds=None):
    """Return valid per-present frame times in ms and skipped row count."""
    times = []
    skipped = 0
    elapsed_ns = 0
    with path.open(encoding="utf-8-sig", errors="replace", newline="") as stream:
        rows = csv.reader(stream)
        # Versioned logs have extra headings. Both formats put hardware
        # metadata before the frame table.
        for row in rows:
            columns = [column.strip().casefold() for column in row]
            if "fps" in columns and "frametime" in columns:
                break
        else:
            return [], skipped
        while not columns[-1]:
            columns.pop()
        time_index = columns.index("frametime")
        elapsed_index = columns.index("elapsed") if "elapsed" in columns else None
        for row in rows:
            if not row:
                continue
            try:
                if len(row) < len(columns) or not row[len(columns) - 1].strip():
                    # An interrupted writer can leave its last row incomplete.
                    skipped += 1
                    continue
                value = float(row[time_index])
            except ValueError:
                skipped += 1
                continue
            if elapsed_index is not None:
                try:
                    elapsed_ns = max(elapsed_ns, int(row[elapsed_index]))
                except ValueError:
                    pass
            if math.isfinite(value) and value > 0:
                times.append(value)
            else:
                skipped += 1
    # A swapchain's first sample can report the monotonic system uptime,
    # sometimes again after valid frames. Genuine stalls can last minutes.
    # Use the whole process lifetime when available, since logging can begin
    # partway through a frame. Archived CSVs carry nanoseconds since log start.
    limit_ms = run_seconds * 1000 if run_seconds is not None else elapsed_ns / 1_000_000
    if limit_ms > 0:
        count = len(times)
        # MangoHud rounds frame times to six significant digits.
        times = [ms for ms in times if ms <= limit_ms or math.isclose(ms, limit_ms, rel_tol=5e-6)]
        skipped += count - len(times)
    return times, skipped


def _read_session(path, run_seconds=None):
    times, skipped = read_frame_times(path, run_seconds)
    if not times:
        return None
    times.sort()
    count = len(times)
    seconds = math.fsum(times) / 1000
    percentiles = {f"p{p}": times[math.ceil(count * p / 100) - 1] for p in (50, 95, 99)}
    return {"csv": path.name, "frames": count, "skipped_rows": skipped,
            "sampled_seconds": seconds, "mean_fps": count / seconds,
            "frametime_ms": {**percentiles, "max": times[-1]}}


def summarize_capture(directory, executable, *, run_seconds=None):
    """Write summary.json for the title's MangoHud CSVs, or return None if empty."""
    directory = Path(directory)
    name = Path(executable).name
    stem = name[:-4] if name.casefold().endswith(".exe") else name
    pattern = re.compile(re.escape(stem) + r"(?:\.exe)?_\d{4}-\d{2}-\d{2}_\d{2}-\d{2}-\d{2}\.csv", re.I)
    sessions = []
    for path in sorted(directory.glob("*")):
        if pattern.fullmatch(path.name) and path.is_file():
            session = _read_session(path, run_seconds)
            if session:
                sessions.append(session)
    if not sessions:
        return None
    result = {"executable": name, "percentile_method": "nearest rank", "sessions": sessions}
    output = directory / "summary.json"
    with TemporaryDirectory(dir=directory) as temp:
        pending = Path(temp) / "summary.json"
        pending.write_text(json.dumps(result, indent=2, allow_nan=False) + "\n", encoding="utf-8")
        pending.replace(output)
    return output
