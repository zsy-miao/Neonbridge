"""Verify a local multi-file v1 torrent and compare its payload with the Xbox layout."""

import argparse
import hashlib
import json
import os
import re
import time
import xml.etree.ElementTree as ET
from datetime import datetime, timezone
from pathlib import Path


MAX_TORRENT_DEPTH = 64


def decode_torrent(data):
    offset = 0

    def read(depth=0):
        nonlocal offset
        token = data[offset:offset + 1]
        offset += 1
        if token == b"i":
            end = data.index(b"e", offset)
            raw = data[offset:end]
            if not re.fullmatch(rb"0|-?[1-9][0-9]*", raw):
                raise ValueError("Invalid bencode integer")
            value = int(raw)
            offset = end + 1
        elif token == b"d":
            if depth >= MAX_TORRENT_DEPTH:
                raise ValueError("Torrent nesting too deep")
            value = {}
            previous_key = None
            while data[offset:offset + 1] != b"e":
                key = read(depth + 1)
                if not isinstance(key, bytes) or (previous_key is not None and key <= previous_key):
                    raise ValueError("Invalid torrent dictionary key")
                previous_key = key
                value[key] = read(depth + 1)
            offset += 1
        elif token == b"l":
            if depth >= MAX_TORRENT_DEPTH:
                raise ValueError("Torrent nesting too deep")
            value = []
            while data[offset:offset + 1] != b"e":
                value.append(read(depth + 1))
            offset += 1
        elif token.isdigit():
            end = data.index(b":", offset)
            raw_size = data[offset - 1:end]
            if not re.fullmatch(rb"0|[1-9][0-9]*", raw_size):
                raise ValueError("Invalid bencode string length")
            size = int(raw_size)
            offset = end + 1 + size
            if offset > len(data):
                raise ValueError("Truncated torrent string")
            value = data[end + 1:offset]
        else:
            raise ValueError("Invalid bencode token")
        return value

    result = read()
    if offset != len(data):
        raise ValueError("Trailing torrent data")
    return result


def inventory(root):
    files = {}
    for path in root.rglob("*"):
        if path.is_file():
            name = path.relative_to(root).as_posix()
            stat = path.stat()
            files[name] = (stat.st_size, stat.st_mtime_ns)
    return files


def verify_pieces(root, entries, sizes, piece_length, hashes):
    total = sum(row["bytes"] for row in entries)
    count = (total + piece_length - 1) // piece_length
    if len(hashes) != count * 20:
        raise ValueError("Piece count disagrees with torrent lengths")
    issues = []
    digest = hashlib.sha1()
    offset = piece = 0
    readable = True
    contributors = []
    last_progress = time.monotonic()
    for row in entries:
        name = row["path"]
        remaining = row["bytes"]
        stream = None
        if sizes.get(name) == remaining:
            stream = (root / name).open("rb")
        try:
            while remaining:
                if not contributors or contributors[-1] != name:
                    contributors.append(name)
                length = min(remaining, piece_length - offset % piece_length, 4 << 20)
                if stream is None:
                    readable = False
                else:
                    block = stream.read(length)
                    if len(block) != length:
                        raise OSError(f"Unexpected end of {name}")
                    digest.update(block)
                offset += length
                remaining -= length
                if offset % piece_length == 0 or offset == total:
                    expected = hashes[piece * 20:(piece + 1) * 20]
                    if not readable or digest.digest() != expected:
                        issues.append({
                            "index_zero_based": piece, "files": contributors,
                            "expected_sha1": expected.hex(),
                            "actual_sha1": digest.hexdigest() if readable else None,
                        })
                    digest = hashlib.sha1()
                    readable = True
                    contributors = []
                    piece += 1
                if time.monotonic() - last_progress >= 30:
                    print(f"{offset / total:.1%} | {piece}/{count} pieces | {len(issues)} issues", flush=True)
                    last_progress = time.monotonic()
        finally:
            if stream is not None:
                stream.close()
    return {"piece_bytes": piece_length, "total": count, "matched": count - len(issues), "issues": issues}


def read_layout_chunks(path):
    """Return each layout chunk with its marker and destination files."""
    chunks = []
    for chunk in ET.parse(path).getroot().findall("Chunk"):
        files = []
        for group in chunk.findall("FileGroup"):
            parts = group.attrib["DestinationPath"].replace("\\", "/").split("/")
            directory = "/".join(part for part in parts if part not in ("", "."))
            files.append("/".join(filter(None, [directory, group.attrib["Include"]])))
        chunks.append({"id": chunk.attrib["Id"], "marker": chunk.get("Marker"), "files": files})
    return chunks


def layout_targets(chunks):
    targets = {}
    for chunk in chunks:
        for name in chunk["files"]:
            targets.setdefault(name, []).append(chunk["id"])
    return targets


def read_layout(path):
    chunks = read_layout_chunks(path)
    return {chunk["id"]: chunk["marker"] for chunk in chunks}, layout_targets(chunks)


def compare_layout(path, payload, disk):
    chunks = read_layout_chunks(path)
    targets = layout_targets(chunks)
    absent_from_torrent = {p: targets[p] for p in sorted(targets.keys() - payload)}
    absent_from_disk = {p: targets[p] for p in sorted(targets.keys() - disk)}
    summary = []
    for chunk in chunks:
        summary.append({
            "id": chunk["id"],
            "marker": chunk["marker"],
            "files": len(chunk["files"]),
            "absent_from_torrent": [name for name in chunk["files"] if name in absent_from_torrent],
            "absent_from_disk": [name for name in chunk["files"] if name in absent_from_disk],
        })
    return {
        "source": path.name, "chunks": len(chunks), "target_count": len(targets),
        "absent_from_torrent": absent_from_torrent,
        "absent_from_disk": absent_from_disk,
        "torrent_paths_outside_layout": sorted(payload - targets.keys()),
        "chunk_summary": summary,
        "launch_marker": next((chunk["id"] for chunk in summary if chunk["marker"] == "Launch"), None),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    try:
        root = args.directory.resolve(strict=True)
    except OSError as exc:
        parser.error(f"cannot read {args.directory}: {exc.strerror or exc}")
    destination = args.output or Path(__file__).resolve().parents[1] / "out" / "integrity" / (root.name + ".json")
    if destination.resolve().is_relative_to(root):
        parser.error("Choose an output outside the input directory")

    def manifest(pattern: str) -> Path:
        matches = [path for path in root.glob(pattern) if path.is_file()]
        if len(matches) != 1:
            parser.error(f"expected exactly one {pattern} file in {root}, found {len(matches)}")
        return matches[0]

    torrent = manifest("*.torrent")
    layout = manifest("layout*.xml")
    start = datetime.now(timezone.utc)
    try:
        raw = torrent.read_bytes()
        info = decode_torrent(raw)[b"info"]
        entries = [{"path": "/".join(s.decode("utf-8") for s in f[b"path"]), "bytes": f[b"length"]}
                   for f in info[b"files"]]
        payload = {row["path"] for row in entries}
        piece_length, hashes = info[b"piece length"], info[b"pieces"]
        if not isinstance(hashes, bytes):
            raise ValueError("Invalid torrent piece hashes")
        if len(payload) != len(entries) or any(row["bytes"] < 0 for row in entries) or not isinstance(piece_length, int) or piece_length <= 0:
            raise ValueError("Invalid torrent file list or piece length")
    except (OSError, ValueError, KeyError, TypeError, AttributeError) as exc:
        parser.error(f"{torrent.name} is not a readable multi-file torrent: {type(exc).__name__}: {exc}")
    before = inventory(root)
    sizes = {p: stat[0] for p, stat in before.items()}
    missing = sorted(payload - sizes.keys())
    wrong_size = [row | {"disk_bytes": sizes[row["path"]]} for row in entries
                  if row["path"] in sizes and sizes[row["path"]] != row["bytes"]]
    report = {
        "started_utc": start.isoformat(), "directory": root.name, "torrent": torrent.name,
        "torrent_sha256": hashlib.sha256(raw).hexdigest(),
        "payload_bytes": sum(row["bytes"] for row in entries), "payload_count": len(entries),
        "disk_file_count": len(sizes), "missing": missing, "size_mismatches": wrong_size,
        "extra_disk_files": sorted(sizes.keys() - payload),
        "layout": compare_layout(layout, payload, sizes.keys()), "torrent_order": entries,
    }
    print(json.dumps({k: report[k] for k in ("payload_count", "payload_bytes", "missing", "size_mismatches", "layout")}), flush=True)
    try:
        report["pieces"] = verify_pieces(root, entries, sizes, piece_length, hashes)
    except ValueError as exc:
        parser.error(f"{torrent.name} is not a readable multi-file torrent: {exc}")
    after = inventory(root)
    changed = sorted(p for p in before.keys() | after.keys() if before.get(p) != after.get(p))
    report["changed_during_verification"] = changed
    report["torrent_payload_passed"] = not (missing or wrong_size or report["pieces"]["issues"] or changed)
    report["finished_utc"] = datetime.now(timezone.utc).isoformat()
    destination.parent.mkdir(parents=True, exist_ok=True)
    temporary = destination.with_suffix(f".{os.getpid()}.tmp")
    temporary.write_text(json.dumps(report, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    temporary.replace(destination)
    print(f"Payload passed: {report['torrent_payload_passed']} | Report: {destination}", flush=True)
    return 0 if report["torrent_payload_passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
