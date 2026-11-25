"""Compile packaged shader programs and verify each Xbox pipeline on the selected GPU."""
from concurrent.futures import ThreadPoolExecutor
import hashlib
from pathlib import Path
import sys
from tempfile import TemporaryDirectory

from check_frosty_shaders import containers, decompress
from project_tuxbox.inspect import digest
from project_tuxbox.shader_prep import compiler_tools, run_tool


def guest(host, exe, adapter, flag, expected):
    receipt = exe.with_name("result.txt")
    receipt.unlink(missing_ok=True)
    run = host.run(exe, [*flag, "--adapter", adapter, "--report", receipt.name], capture=True, timeout=120)
    line = receipt.read_text().strip() if receipt.exists() else ""
    if run.returncode or line != expected:
        sys.stderr.buffer.write(run.stdout[-4000:])
        raise ValueError(f"Expected {expected!r}, got exit {run.returncode} and {line!r}")
    return line


def check_packaged_pipelines(payload, entries, decode, host, adapter, trial, exe, indices, profiles, name):
    xshader, dxc, dxv = compiler_tools()
    cache = trial / "temp/tuxbox_shader_cache/by_source"
    cache.mkdir(parents=True, exist_ok=True)

    def prepare(index):
        ((_, blob, _),) = containers(decompress(payload, entries[index], decode))
        source = trial / f"{index}.dxbc"
        source.write_bytes(blob)
        source_hash = digest(source)
        with TemporaryDirectory(dir=trial) as tmp:
            hlsl, profile, dxil = (Path(tmp) / name for name in ("shader.hlsl", "profile", "shader.cso"))
            run_tool([xshader, source, "--hlsl-out", hlsl, "--profile-out", profile])
            target = profile.read_text().strip()
            if target not in profiles:
                raise ValueError(f"Entry {index} produced {target}, expected one of {sorted(profiles)}")
            run_tool([dxc, "-T", target, "-E", "main", "-Fo", dxil, hlsl])
            run_tool([dxv, dxil])
            dxil_hash = digest(dxil)
            dxil.replace(cache / (source_hash + ".cso"))
        return f"{index} {source_hash} {dxil_hash}"

    with ThreadPoolExecutor(max_workers=4) as workers:
        rows = list(workers.map(prepare, indices))
    (trial / "pipelines.txt").write_text("".join(f"{index}\n" for index in indices))
    line = guest(host, exe, adapter, ["--pipelines"],
                 f"{name}-pipelines PASS pci={adapter} created={len(indices)}")
    results = (trial / "pipeline-results.txt").read_text().splitlines()
    if not indices or results != [f"{index} 00000000" for index in indices]:
        raise ValueError("Missing or failed Xbox pipeline in the per-entry results")
    fingerprint = hashlib.sha256(("\n".join(rows) + "\n").encode("ascii")).hexdigest()
    return {"receipt": line, "archive_entries": indices, "compiler_sha256": digest(xshader),
            "dxc_sha256": digest(dxc), "dxv_sha256": digest(dxv), "program_manifest_sha256": fingerprint,
            "manifest_format": "Archive index, source DXBC SHA-256 and validated DXIL SHA-256 in archive order, separated by spaces, one LF per row."}
