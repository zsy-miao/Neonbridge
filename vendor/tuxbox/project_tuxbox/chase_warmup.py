"""Prepare the retail chase programs and linked variants in the on-demand compiler's cache."""
from concurrent.futures import ThreadPoolExecutor, as_completed
import ctypes
import hashlib
import os
from pathlib import Path
import shutil
from tempfile import TemporaryDirectory

from .inspect import digest
from .prep_progress import PrepPhase, report_progress
from .runtime_shaders import MAX_SHADER_BYTES, compile_request, portable_stage, snapshot_compiler_cache
from .shader_prep import run_tool
from .shader_inventory import ARCHIVES, ENTRY, STAGES, decoder, dxbc, parse_tables, serialized_state


def extract_programs(stream, archive, indices, work, decode):
    offset, size, expected = ARCHIVES[archive]
    stream.seek(offset)
    member = memoryview(stream.read(size))
    if len(member) != size or hashlib.sha256(member).hexdigest() != expected:
        raise ValueError(f"Unexpected {archive} archive fingerprint")
    _, _, descriptors, _, payload = parse_tables(member)
    sources = {}
    for index in indices:
        if type(index) is not int or not 0 <= index < len(descriptors) // ENTRY.size:
            raise ValueError(f"Invalid {archive} chase shader index {index!r}")
        ref = f"{archive}/{index}"
        start, stored, length, stage = ENTRY.unpack_from(descriptors, index * ENTRY.size)
        if stage >= len(STAGES) or not 0 < stored <= length <= MAX_SHADER_BYTES or start + stored > len(payload):
            raise ValueError(f"Invalid chase shader descriptor {ref}")
        cooked = bytes(payload[start:start + stored])
        if stored != length:
            raw = ctypes.create_string_buffer(length)
            if decode(cooked, raw, stored, length) != length:
                raise ValueError(f"Unable to decompress chase shader {ref}")
            cooked = raw.raw
        source = serialized_state(cooked)[1] if stage >= 6 else dxbc(cooked)
        key = hashlib.sha256(source).hexdigest()
        (work / (key + ".dxbc")).write_bytes(source)
        sources[ref] = (key, stage)
    return sources


def prepare_batch(jobs, prepare, label, progress):
    compiled = 0
    if not jobs:
        return 0
    phase = PrepPhase(progress, label, len(jobs))
    with ThreadPoolExecutor(max_workers=2) as pool:
        pending = {pool.submit(prepare, job): job for job in jobs}
        try:
            for ordinal, result in enumerate(as_completed(pending), 1):
                try:
                    built = result.result()
                except (ValueError, OSError) as error:
                    raise ValueError(f"Preparing chase {label} {pending[result]}: {error}") from error
                compiled += built
                phase.update(ordinal, compiled)
        finally:
            for future in pending:
                future.cancel()
    return compiled


def prepare_chase(pak, by_source, profile, title_sha256, prepared_sources, progress=None, *, prepare_host=None):
    import fcntl  # The runtime's Proton prefix and file locking are Linux-specific.

    if profile["title_sha256"] != title_sha256 or profile["archives"] != {k: v[2] for k, v in ARCHIVES.items()}:
        raise ValueError("Chase shader profile does not match the title and archives")
    if profile["programs"].keys() != ARCHIVES.keys():
        raise ValueError("Chase shader program list must name both archives")
    root = by_source.parent
    # Serialize cold starts for a prefix. Each completed program is published by
    # compile_request, so a later failure or cancellation keeps the valid work.
    with ((root / "tuxbox-first-boot.lock").open("a+b") as lock,
          TemporaryDirectory(dir=root, prefix=".chase-") as tmp,
          TemporaryDirectory(prefix="tuxbox-xshader-") as host_tmp):
        fcntl.flock(lock, fcntl.LOCK_EX)
        work = Path(tmp)
        report_progress(progress, "Checking the chase shader archives")
        tools, cache = snapshot_compiler_cache(Path(host_tmp) / "xshader", root)
        sources = {}
        decode = decoder()
        with pak.open("rb") as stream:
            for archive, indices in profile["programs"].items():
                sources.update(extract_programs(stream, archive, indices, work, decode))
        programs = dict(sources.values())
        pairs = set()
        for producer, pixel in profile["links"]:
            if (producer not in sources or pixel not in sources
                    or sources[producer][1] not in (0, 4) or sources[pixel][1] != 3):
                raise ValueError(f"Invalid chase shader link {producer} / {pixel}")
            pairs.add((sources[producer][0], sources[pixel][0]))

        # The provider checks by_source before the versioned runtime cache.
        # The regular first-boot step already authenticated its own entries.
        # Other staged programs must also match the expected desktop stage.
        def prepare_program(key):
            dest = work / (key + ".cso")
            staged = by_source / dest.name
            if key in prepared_sources:
                shutil.copyfile(staged, dest)
                return False
            if staged.is_file() and staged.stat().st_size <= MAX_SHADER_BYTES:
                with staged.open("rb") as stream:
                    code = stream.read(MAX_SHADER_BYTES + 1)
                # UE archive frequencies: VS, MS, AS, PS, GS, CS, then DXR libraries.
                stage = programs[key]
                expected = 6 if stage >= 6 else (1, 13, 14, 0, 2, 5)[stage]
                if len(code) <= MAX_SHADER_BYTES and portable_stage(code) == expected:
                    dest.write_bytes(code)
                    run_tool([tools[2], dest], timeout=10)
                    return False
            return compile_request(work / (key + ".dxbc"), key, tools, cache, work)

        compiled = prepare_batch(sorted(key for key, stage in programs.items() if stage < 6),
                                 prepare_program, "Chase shaders", progress)
        compiled += prepare_batch(sorted(key for key, stage in programs.items() if stage >= 6),
                                  prepare_program, "Ray-tracing libraries", progress)

        def prepare_link(pair):
            producer, pixel = pair
            pixel_dxil = work / (pixel + ".cso")
            key = f"{producer}_{pixel}_{digest(pixel_dxil)}"
            request = work / (key + ".link")
            request.mkdir()
            os.link(work / (producer + ".dxbc"), request / "producer.dxbc")
            os.link(work / (pixel + ".dxbc"), request / "pixel.dxbc")
            os.link(pixel_dxil, request / "pixel.cso")
            return compile_request(request, key, tools, cache, work)

        compiled += prepare_batch(sorted(pairs), prepare_link, "Linked chase shaders", progress)
        stencils = set()
        for ref, slot in profile.get("stencil_targets", ()):
            if ref not in sources or sources[ref][1] != 3 or type(slot) is not int or not 0 <= slot < 8:
                raise ValueError(f"Invalid chase stencil target {ref} / {slot}")
            stencils.add((sources[ref][0], slot))

        def prepare_stencil(target):
            source, slot = target
            key = f"{source}_rt{slot}"
            request = work / (key + ".stencil")
            os.link(work / (source + ".dxbc"), request)
            return compile_request(request, key, tools, cache, work)

        compiled += prepare_batch(sorted(stencils), prepare_stencil, "Stencil variants", progress)
        if prepare_host:
            prepare_host(work, sources)
        return compiled, len(programs) + len(pairs) + len(stencils) - compiled
