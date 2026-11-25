"""Compile ProjectScarlett's recorded shader requests before launching the title."""
from collections import Counter
from concurrent.futures import FIRST_COMPLETED, ThreadPoolExecutor, wait
import os
from itertools import islice
import json
from pathlib import Path
import re
import subprocess
from tempfile import TemporaryDirectory
from time import monotonic

from . import dxil_store
from .inspect import digest
from .prep_progress import PrepPhase, desktop_progress, report_progress
from .runtime_shaders import REQUEST_PATTERNS, ShaderRefused, compile_request, snapshot_compiler_cache
from .shader_prep import SOURCE_CACHE
from .shader_diagnostics import normalize_refusal
from .shader_pack import seed_requests
from .scarlett_report import aggregate, refusal_bucket, failure_bucket


def scarlett_requests(executable, cache):
    if executable.name.casefold() != "game_scarlett_master.exe" or not executable.with_name("shaders.rpf").is_file():
        return None
    return cache / "projectscarlett" / digest(executable) / "requests"


PREP_JOBS_LIMIT = 32


def prep_jobs(env, report=report_progress):
    """Compiler jobs for launch-time preparation.

    Preparation runs before the game starts, so it can use more of the CPU
    than live compilation (TUXBOX_RUNTIME_SHADER_JOBS). Default to the
    available cores, at most 8; TUXBOX_SHADER_PREP_JOBS chooses 1 to 32.
    """
    try:
        cpus = len(os.sched_getaffinity(0))
    except (AttributeError, OSError):
        cpus = os.cpu_count() or 1
    default = max(1, min(8, cpus))
    value = env.get("TUXBOX_SHADER_PREP_JOBS")
    if value is None:
        return default
    try:
        jobs = int(value)
        if not 1 <= jobs <= PREP_JOBS_LIMIT:
            raise ValueError
    except ValueError:
        report(None, f"Invalid TUXBOX_SHADER_PREP_JOBS={value!r}; using {default}")
        return default
    return jobs


def seed_from_pack(executable, archive, env):
    """With TUXBOX_SCARLETT_SEED_PACK=1, add every shader in shaders.rpf to the
    request archive so the first launch prepares more than earlier runs saw."""
    if env.get("TUXBOX_SCARLETT_SEED_PACK") != "1":
        return
    report_progress(None, "Reading ProjectScarlett's shader pack")
    started = monotonic()
    try:
        found, added = seed_requests(executable.with_name("shaders.rpf"), archive,
                                     lambda message: report_progress(None, message))
    except OSError as exc:
        report_progress(None, f"Could not read ProjectScarlett's shader pack: {exc}")
        return
    report_progress(None, f"Shader pack: {found} shaders found, {added} added to preparation "
                    f"in {monotonic() - started:.1f}s.")


def bounded_results(executor, compile_one, sources, jobs):
    """Keep at most twice the worker count in flight, including queued jobs."""
    if jobs < 1:
        raise ValueError("at least one preparation worker required")
    remaining = iter(sources)
    pending = {executor.submit(compile_one, source) for source in islice(remaining, jobs * 2)}
    while pending:
        done, pending = wait(pending, return_when=FIRST_COMPLETED)
        # Drop the completed futures as they are consumed; never enqueue an
        # entire title's request archive. Refill only as capacity is freed.
        while done:
            result = done.pop().result()
            for source in islice(remaining, 1):
                pending.add(executor.submit(compile_one, source))
            yield result


def prepare_scarlett(executable, host):
    if (host.env.get("TUXBOX_RUNTIME_SHADERS") == "0"
            or not executable.with_name("d3d12_xs.dll").is_file()):
        return False
    cache_root = Path(host.env["WINEPREFIX"]) / SOURCE_CACHE.parent
    archive = scarlett_requests(executable, cache_root)
    if archive is None:
        return False
    seed_from_pack(executable, archive, host.env)
    sources = sorted(
        path for path in archive.iterdir()
        if path.suffix in REQUEST_PATTERNS and re.fullmatch(REQUEST_PATTERNS[path.suffix], path.stem)
    ) if archive.is_dir() else []
    if not sources:
        report_progress(None, "ProjectScarlett shader preparation will start after the game records its first shader requests.")
        return True

    started = monotonic()
    reused_before = dxil_store.reused["dxil"]
    totals = Counter()
    reasons = Counter()
    refusal_counts, failure_counts, request_counts = Counter(), Counter(), Counter()
    # Use the runtime toolchain and cache so the guest can load both translations
    # and deterministic refusals directly, including requests with a pipeline root.
    env = {"TUXBOX_SHADER_PREP_UI": "1", **host.env}
    with desktop_progress("ProjectScarlett", env) as progress, TemporaryDirectory(prefix="tuxbox-prep-") as tmp:
        report_progress(progress, "Preparing ProjectScarlett shaders")
        tools, cache = snapshot_compiler_cache(Path(tmp) / "xshader", cache_root)
        phase = PrepPhase(progress, "Previously used Xbox shaders", len(sources))

        def compile_one(source):
            try:
                compiled = compile_request(source, source.stem, tools, cache, None)
                return ("compiled" if compiled else "cached"), None, None, source.suffix, None
            except ShaderRefused as exc:
                return "unsupported", None, refusal_bucket(exc), source.suffix, normalize_refusal(str(exc)) if exc.reason_known else None
            except (OSError, ValueError, subprocess.SubprocessError) as exc:
                return "failed", f"{source.name}: {exc}", failure_bucket(exc), source.suffix, None

        jobs = prep_jobs(host.env)
        workers = ThreadPoolExecutor(max_workers=jobs)
        try:
            for completed, result in enumerate(bounded_results(workers, compile_one, sources, jobs), 1):
                category, error, bucket, suffix, reason = result
                totals[category] += 1
                request_counts[suffix] += 1
                if category == "unsupported":
                    refusal_counts[bucket] += 1
                elif category == "failed":
                    failure_counts[bucket] += 1
                if reason is not None:
                    reasons[reason] += 1
                if error and totals["failed"] <= 3:
                    report_progress(progress, f"ProjectScarlett shader preparation: {error}", completed, len(sources))
                phase.update(completed, totals["compiled"],
                             unsupported=totals["unsupported"], failed=totals["failed"])
        finally:
            workers.shutdown(cancel_futures=True)

        elapsed = monotonic() - started
        unsupported = totals["unsupported"]
        report_progress(progress, f"ProjectScarlett shader preparation took {elapsed:.1f}s. "
                        f"{totals['compiled']} compiled, {totals['cached']} cached, "
                        f"{totals['unsupported']} unsupported, {totals['failed']} failed.", 1, 1)
        reused = dxil_store.reused["dxil"] - reused_before
        if reused:
            report_progress(None, f"ProjectScarlett DXIL reused for {reused} of the compiled shaders "
                            "(unchanged since an earlier translator).")
        receipt = aggregate(totals, refusal_counts, failure_counts, request_counts)
        report_progress(None, "ProjectScarlett preparation aggregate: " +
                        json.dumps(receipt, sort_keys=True, separators=(",", ":")))
    if unsupported:
        unknown = refusal_counts['cached_reason_unavailable']
        report_progress(None, f"ProjectScarlett shader refusals: {totals['unsupported'] - unknown} with a reason, "
                        f"{unknown} cached before reasons were kept.")
        for reason, count in sorted(reasons.items(), key=lambda item: (-item[1], item[0]))[:10]:
            report_progress(None, f"  {count} x {reason}")
    return True
