"""Compile Xbox shaders and link raster stages requested by a running Proton game."""
from contextlib import contextmanager, nullcontext
from concurrent.futures import FIRST_COMPLETED, ThreadPoolExecutor, wait as wait_futures
import errno
import hashlib
import os
from pathlib import Path
import re
import shutil
import stat
import struct
import subprocess
from tempfile import TemporaryDirectory
from threading import Event, Lock, Thread
from time import monotonic, sleep

from .dxil_store import DxilStore, write_identity
from .inspect import digest
from .persistent_dxc import PersistentDxc
from .shader_diagnostics import refusal_line
from .shader_prep import SOURCE_CACHE, ToolFailure, compiler_tools, run_tool


MAX_SHADER_BYTES = 64 << 20
XSHADER_UNSUPPORTED_HLSL = 65
# The emitter's one-line refusal reason kept beside an .unsupported marker.
REASON_BYTES = 512
SOURCE_KEY = r"[a-f0-9]{64}"
STENCIL_KEY = SOURCE_KEY + r"_rt[0-7]"
LINK_KEY = "_".join([SOURCE_KEY] * 3)
REQUEST_PATTERNS = {".link": LINK_KEY, ".stencil": STENCIL_KEY, ".dxbc": SOURCE_KEY}


class ShaderRefused(ValueError):
    def __init__(self, message, *, cached, remembered=False):
        super().__init__(message)
        self.cached = cached
        # A cached refusal whose emitter reason was kept beside its marker.
        self.remembered = remembered

    @property
    def reason_known(self):
        return not self.cached or self.remembered


def bounded_shader(path):
    """Snapshot a regular file without following links or blocking on FIFOs."""
    flags = os.O_RDONLY | getattr(os, "O_NONBLOCK", 0) | getattr(os, "O_NOFOLLOW", 0)
    with os.fdopen(os.open(path, flags), "rb") as stream:
        info = os.fstat(stream.fileno())
        if not stat.S_ISREG(info.st_mode):
            raise ValueError(f"{path.name} is not a regular shader file")
        if info.st_size > MAX_SHADER_BYTES:
            raise ValueError("shader exceeds the 64 MiB runtime limit")
        data = stream.read(MAX_SHADER_BYTES + 1)
    if len(data) > MAX_SHADER_BYTES:
        raise ValueError("shader exceeds the 64 MiB runtime limit")
    return data


def verified_input(path, expected):
    data = bounded_shader(path)
    if hashlib.sha256(data).hexdigest() != expected:
        raise ValueError(f"{path.name} contents differ from the requested digest")
    return data


STAGE_IDS = {"ps": 0, "vs": 1, "gs": 2, "cs": 5, "lib": 6, "ms": 13, "as": 14}


def container_stage(data, *, portable):
    """Match the runtime's container/stage gate, not just LLVM magic."""
    if len(data) < 32 or len(data) > MAX_SHADER_BYTES or data[:4] != b"DXBC":
        return None
    size, count = struct.unpack_from("<II", data, 24)
    end = 32 + count * 4
    if size != len(data) or end > size:
        return None
    program = None
    accc = psv = 0
    for offset, in struct.iter_unpack("<I", memoryview(data)[32:end]):
        if offset < end or offset + 8 > size:
            return None
        tag, length = struct.unpack_from("<4sI", data, offset)
        start = offset + 8
        if start + length > size:
            return None
        accc += tag == b"ACCC"
        psv += tag == b"PSV0"
        if tag == b"DXIL":
            if program is not None:
                return None
            program = memoryview(data)[start:start + length]
    if program is None:
        # ACCC-only console compute. Full extraction still belongs to xshader.
        return 5 if not portable and accc == 1 and psv == 1 else None
    if len(program) < 24:
        return None
    version, words, magic, _, offset, length = struct.unpack_from("<II4sIII", program)
    stage = version >> 16
    start = 8 + offset
    if (stage not in STAGE_IDS.values() or version & 0xf0 != 0x60
            or words * 4 != len(program) or magic != b"DXIL"
            or start < 24 or start + length > len(program)):
        return None
    bits = program[start:start + length]
    if portable:
        return stage if bits[:4] == b"BC\xc0\xde" else None
    return stage if bits[:4] == b"BC\xc0\xde" or (program[24:28] == b"XDAT" and bits == b"NOBC") else None


def portable_stage(data):
    return container_stage(data, portable=True)


def exact_marker(path, expected):
    """Read a tiny regular marker, without following links or blocking."""
    flags = os.O_RDONLY | getattr(os, "O_NONBLOCK", 0) | getattr(os, "O_NOFOLLOW", 0)
    try:
        with os.fdopen(os.open(path, flags), "rb") as stream:
            info = os.fstat(stream.fileno())
            return (stat.S_ISREG(info.st_mode) and info.st_size == len(expected)
                    and stream.read(len(expected) + 1) == expected)
    except OSError:
        return False


def remembered_reason(path):
    """The refusal reason saved with a marker, or None for caches that predate it."""
    flags = os.O_RDONLY | getattr(os, "O_NONBLOCK", 0) | getattr(os, "O_NOFOLLOW", 0)
    try:
        with os.fdopen(os.open(path, flags), "rb") as stream:
            info = os.fstat(stream.fileno())
            if not stat.S_ISREG(info.st_mode) or info.st_size > REASON_BYTES:
                return None
            data = stream.read(REASON_BYTES + 1)
    except OSError:
        return None
    if len(data) > REASON_BYTES:
        return None
    return data.decode("utf-8", errors="replace").strip() or None


@contextmanager
def compile_lock(cache, key, timeout=90):
    """Serialize an exact cold cache key across independent launcher processes.

    Keep the lock inode for this compiler fingerprint: unlinking it on
    release could let a later process lock a different inode while another
    waiter still holds the original. No locks are acquired for warm hits.
    """
    import fcntl
    if not any(re.fullmatch(pattern, key) for pattern in REQUEST_PATTERNS.values()):
        raise ValueError("invalid shader cache key")
    directory = cache / ".locks"
    directory.mkdir(exist_ok=True)
    flags = os.O_RDWR | os.O_CREAT | getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_NONBLOCK", 0)
    with os.fdopen(os.open(directory / key, flags, 0o600), "r+b") as stream:
        if not stat.S_ISREG(os.fstat(stream.fileno()).st_mode):
            raise ValueError("shader cache lock is not a regular file")
        deadline = monotonic() + timeout
        while True:
            try:
                fcntl.flock(stream, fcntl.LOCK_EX | fcntl.LOCK_NB)
                break
            except BlockingIOError:
                if monotonic() >= deadline:
                    raise TimeoutError("shader cache lock wait timed out")
                sleep(min(.025, max(0, deadline - monotonic())))
        try:
            yield
        finally:
            fcntl.flock(stream, fcntl.LOCK_UN)


def cache_snapshot(dest, checksum, stages):
    """Return only bounded, stage-correct bytes with their matching checksum."""
    try:
        data = bounded_shader(dest)
        if portable_stage(data) in stages and exact_marker(checksum, hashlib.sha256(data).hexdigest().encode("ascii")):
            return data
    except (OSError, ValueError):
        pass
    return None


def portable_pixel(data):
    return portable_stage(data) == 0  # D3D12_SHVER_PIXEL_SHADER


def archive_request(archive, source, inputs):
    """Publish the exact verified inputs for a later launch's compiler."""
    dest = archive / source.name
    if dest.exists():
        return
    try:
        if source.suffix == ".link":
            staged = inputs[0].parent / "record"
            staged.mkdir()
            for path, name in zip(inputs, ("producer.dxbc", "pixel.dxbc", "pixel.cso")):
                os.link(path, staged / name)
            staged.rename(dest)
        else:
            os.link(inputs[0], dest)
    except OSError as exc:
        if exc.errno not in (errno.EEXIST, errno.ENOTEMPTY):
            raise


# Unoptimised DXC is the last resort for a shader. The largest ProjectScarlett
# compute program (CS_Emit_Dynamic_Model_Edge, 7,813 instructions) needs
# about 18 s at -O0, so a 15 s cap refused it on every boot.
O0_TIMEOUT = 90


def validated_dxil(dxv, output, stage):
    """Return bounded, stage-correct output bytes after DXV accepts the file."""
    run_tool([dxv, output], timeout=10)
    data = bounded_shader(output)
    if portable_stage(data) != stage:
        raise ValueError("compiler returned invalid DXIL or a different shader stage")
    return data


def compile_dxil(dxc, dxv, target, hlsl, output, store, request, stage):
    """Compile HLSL for the requested stage and return fully validated DXIL bytes."""
    entry = [] if target == "lib_6_3" else ["-E", "main"]
    unoptimised = [dxc, "-T", target, *entry, "-O0", "-Fo", output, hlsl]
    # Large generated pixel programs can spend the cap in Jump Threading.
    # Try this validated variant before consulting the baseline slow marker;
    # an older DXC's flag rejection falls back just like a candidate timeout.
    if target == "ps_6_6" and hlsl.stat().st_size >= (256 << 10):
        try:
            run_tool([dxc, "-T", target, *entry, "-disable-lifetime-markers",
                      "-Fo", output, hlsl], timeout=15)
            if not output.exists():
                raise ValueError("compiler returned no DXIL output")
            return validated_dxil(dxv, output, stage)
        except (subprocess.TimeoutExpired, ValueError):
            output.unlink(missing_ok=True)
    # cs_6_6 programs from this boot either finish default optimisation in
    # under a second or sit in it until the cap. The ones that sit there
    # compile at -O0 in a few seconds, and the driver optimises that DXIL
    # again when it builds the pipeline. A request that timed out before goes
    # straight to -O0 rather than spending the cap again.
    if target == "cs_6_6" or (store and store.is_slow(request, target)):
        run_tool(unoptimised, timeout=O0_TIMEOUT)
        return validated_dxil(dxv, output, stage)
    try:
        run_tool([dxc, "-T", target, *entry, "-Fo", output, hlsl], timeout=15)
    except subprocess.TimeoutExpired:
        if store:
            store.mark_slow(request, target)
        output.unlink(missing_ok=True)
        run_tool(unoptimised, timeout=O0_TIMEOUT)
    return validated_dxil(dxv, output, stage)


def compile_request(source, key, tools, cache, session, *, remember=None, persistent=None):
    linked = source.suffix == ".link"
    xshader, dxc, dxv = tools
    with TemporaryDirectory(dir=cache) as tmp:
        work = Path(tmp)
        flags = []
        if linked:
            if not stat.S_ISDIR(source.lstat().st_mode):
                raise ValueError("linked shader request must be a directory")
            producer_hash, pixel_hash, dxil_hash = key.split("_")
            cooked = work / "producer.dxbc"
            source_data = verified_input(source / "producer.dxbc", producer_hash)
            cooked.write_bytes(source_data)
            pixel = work / "pixel.dxbc"
            pixel.write_bytes(verified_input(source / "pixel.dxbc", pixel_hash))
            pixel_code = verified_input(source / "pixel.cso", dxil_hash)
            if not portable_pixel(pixel_code):
                raise ValueError("pixel order requires a portable pixel DXIL container")
            order = work / "pixel.cso"
            order.write_bytes(pixel_code)
            flags = ["--link-pixel", pixel, "--pixel-order", order]
            inputs = (cooked, pixel, order)
        else:
            cooked = work / "input.dxbc"
            if source.suffix == ".stencil":
                source_hash, slot = key.split("_rt")
                flags = ["--stencil-target", slot]
            else:
                source_hash = key
            source_data = verified_input(source, source_hash)
            cooked.write_bytes(source_data)
            inputs = (cooked,)

        if remember:
            remember(source, inputs)

        dest = cache / (key + ".cso")
        checksum = dest.with_suffix(".sha256")
        refusal = dest.with_suffix(".unsupported")
        reply = work / "reply"
        stages = {1, 2, 13} if linked else {0} if source.suffix == ".stencil" else set(STAGE_IDS.values())
        expected_stage = container_stage(source_data, portable=False)
        if expected_stage is not None:
            stages &= {expected_stage}
        del source_data
        def reuse():
            cached = cache_snapshot(dest, checksum, stages)
            if cached is not None:
                # Prelaunch hits need no temporary DXIL copy.
                if session is not None:
                    reply.write_bytes(cached)
                    os.replace(reply, session / dest.name)
                return True
            if exact_marker(refusal, key.encode("ascii")):
                reason = remembered_reason(dest.with_suffix(".reason"))
                if reason:
                    raise ShaderRefused(reason, cached=True, remembered=True)
                raise ShaderRefused("cached unsupported HLSL translation", cached=True)
            return False

        if reuse():
            return False
        with compile_lock(cache, key):
            # The first process may have published while we waited.
            if reuse():
                return False
            hlsl, output, profile = (work / name for name in ("shader.hlsl", "out.cso", "profile"))
            if linked:
                run_tool([dxv, order], timeout=10)
            try:
                run_tool([xshader, cooked, *flags, "--hlsl-out", hlsl, "--profile-out", profile], timeout=30)
            except ToolFailure as error:
                # xshader reserves this status for the in-process HLSL emitter. DXC, DXV,
                # input parsing, tool startup and filesystem failures remain retryable.
                if error.returncode != XSHADER_UNSUPPORTED_HLSL:
                    raise
                # Publish the reason first: a visible marker may already be read.
                reason = work / "reason"
                reason.write_bytes(refusal_line(str(error)).encode("utf-8")[:REASON_BYTES])
                os.replace(reason, dest.with_suffix(".reason"))
                marker = work / "unsupported"
                marker.write_text(key, encoding="ascii")
                os.replace(marker, refusal)
                raise ShaderRefused(str(error), cached=False) from error
            target = profile.read_text().strip()
            # Mesh and amplification stages start at shader model 6.5. Linked
            # requests currently repack the outputs of vertex and geometry shaders.
            profiles = r"(vs|gs)_6_[016]|ms_6_[56]" if linked else r"((cs|ps|vs|gs)_6_[016]|cs_6_[45]|(ms|as)_6_[56]|lib_6_3)"
            if not re.fullmatch(profiles, target):
                raise ValueError(f"unsupported compiler profile {target!r}")
            target_stage = STAGE_IDS[target.split("_", 1)[0]]
            if target_stage not in stages:
                raise ValueError("compiler profile differs from requested shader stage")
            # An earlier translator may have produced this exact HLSL. Reuse any
            # validated equivalent output, including the candidate and -O0 paths;
            # compiler policy changes deliberately leave warm cache entries valid.
            store = DxilStore.for_cache(cache)
            content = store.key(hlsl.read_bytes(), target) if store else None
            data = bounded_shader(output) if store and store.fetch(content, output) else None
            if data is None or portable_stage(data) != target_stage:
                output.unlink(missing_ok=True)
                data = None
                if (persistent is not None and target == "cs_6_0"
                        and not (store and store.is_slow(key, target))
                        and persistent.compile(dxc, hlsl, output)):
                    try:
                        data = validated_dxil(dxv, output, target_stage)
                    except (OSError, ValueError, subprocess.SubprocessError) as error:
                        persistent.report(f"Persistent DXC output rejected, using the CLI: {error}")
                        output.unlink(missing_ok=True)
                if data is None:
                    data = compile_dxil(dxc, dxv, target, hlsl, output, store, key, target_stage)
                if store:
                    store.save(content, output)
            stamp = work / "sha256"
            stamp.write_text(hashlib.sha256(data).hexdigest())
            if session is not None:
                reply.write_bytes(data)
            os.replace(output, dest)
            os.replace(stamp, checksum)
            if session is not None:
                os.replace(reply, session / dest.name)
    return True


def remove_request(path):
    if not path.is_symlink() and path.is_dir():
        shutil.rmtree(path)
    else:
        path.unlink(missing_ok=True)


def serve(session, stop, report, archive=None, jobs=1, *, dxc_helper=None):
    # Cargo can replace the release executable while another title is running.
    # Keep the executable we fingerprint private to this host worker.
    with TemporaryDirectory(prefix="tuxbox-xshader-") as tmp, request_notifications(session / "requests", stop) as wait:
        pinned = None
        if dxc_helper:
            try:
                helper = Path(tmp) / "dxc-worker"
                shutil.copyfile(Path(dxc_helper).expanduser(), helper)
                helper.chmod(0o700)
                pinned = helper
            except OSError as error:
                report(f"Could not stage persistent DXC, using the CLI: {error}")
        with PersistentDxc(pinned, jobs, report) if pinned else nullcontext() as persistent:
            _serve_requests(session, stop, report, Path(tmp) / "xshader", wait, archive, jobs, persistent=persistent)


@contextmanager
def request_notifications(directory, stop):
    # The Proton guest publishes both shader files and complete link directories
    # by renaming them into this directory. Install the watch before scanning.
    import ctypes
    import select
    libc = ctypes.CDLL(None)
    init = getattr(libc, "inotify_init1", None)
    fd = init(os.O_NONBLOCK | os.O_CLOEXEC) if init else -1
    if fd < 0:
        yield lambda: stop.wait(0.05)
        return
    try:
        moved_to = 0x00000080
        if libc.inotify_add_watch(fd, os.fsencode(directory), moved_to) < 0:
            yield lambda: stop.wait(0.05)
            return
        changes = select.poll()
        changes.register(fd, select.POLLIN)

        def wait():
            # A bounded fallback scan also handles filesystems without events
            # and lets shutdown finish when there is no outstanding work.
            if changes.poll(50):
                os.read(fd, 65536)

        yield wait
    finally:
        os.close(fd)


def snapshot_compiler_cache(xshader, cache_root, *, dxc_helper=None):
    """Pin the executable used for all requests under this toolchain fingerprint."""
    compiler, dxc, dxv = compiler_tools()
    shutil.copyfile(compiler, xshader)
    xshader.chmod(0o700)
    tools = (xshader, dxc, dxv)
    version = subprocess.check_output([dxc, "--version"], timeout=10)
    if dxc_helper is not None:
        version += b"\npersistent-dxc:" + digest(dxc_helper).encode("ascii")
    fingerprint = hashlib.sha256(version)
    for tool in tools:
        fingerprint.update(bytes.fromhex(digest(Path(tool))))
    cache = cache_root / "runtime" / fingerprint.hexdigest()
    cache.mkdir(parents=True, exist_ok=True)
    write_identity(cache, version, dxc, dxv)
    return tools, cache


def runtime_jobs(env, report):
    """Bound live compiler subprocesses; honor the launcher's CPU affinity."""
    try:
        cpus = len(os.sched_getaffinity(0))
    except (AttributeError, OSError):
        cpus = os.cpu_count() or 1
    cap = max(1, min(8, cpus))
    default = min(4, cap)
    value = env.get("TUXBOX_RUNTIME_SHADER_JOBS")
    if value is None:
        return default
    try:
        jobs = int(value)
        if not 1 <= jobs <= 8:
            raise ValueError
    except ValueError:
        report(f"Invalid TUXBOX_RUNTIME_SHADER_JOBS={value!r}; using {default}")
        return default
    return min(jobs, cap)


def ordered_requests(directory):
    """Mix ordinary, linked and stencil requests in publication order."""
    entries = []
    for source in directory.iterdir():
        if source.suffix not in REQUEST_PATTERNS:
            continue
        try:
            timestamp = source.lstat().st_mtime_ns
        except FileNotFoundError:
            continue
        entries.append((timestamp, source.name, source))
    return [source for _, _, source in sorted(entries)]


def _serve_requests(session, stop, report, xshader, wait, archive, jobs=1, *, persistent=None):
    def bind_cache():
        options = {"dxc_helper": persistent.helper} if persistent is not None else {}
        tools, cache = snapshot_compiler_cache(xshader, session.parent, **options)
        # Publish only after the exact compiler used by this worker is pinned.
        pending = session / "compiler-id.tmp"
        pending.write_text(cache.name, encoding="ascii")
        pending.replace(session / "compiler-id")
        return tools, cache

    tools = cache = None
    if (session.parent / "runtime").is_dir():
        try:
            tools, cache = bind_cache()
        except (OSError, ValueError, subprocess.SubprocessError):
            pass  # Retry and report the error if this game needs a shader.
    refused = cached_refusals = 0
    record_requests = archive is not None
    archive_lock = Lock()
    archive_errors = []

    def remember(source, inputs):
        nonlocal archive
        with archive_lock:
            if archive is not None:
                try:
                    archive_request(archive, source, inputs)
                except OSError as exc:
                    archive_errors.append(f"Could not save ProjectScarlett shader requests for the next launch: {exc}")
                    archive = None

    def compile_one(source, key, bound_tools, bound_cache):
        # Only this job owns the source. Archive publication is separately
        # serialized; compiler temporary directories are private per request.
        options = {"remember": remember} if record_requests else {}
        if persistent is not None:
            options["persistent"] = persistent
        return compile_request(source, key, bound_tools, bound_cache, session, **options)

    def complete(source, key, future=None, error=None):
        nonlocal refused, cached_refusals
        failed = False
        try:
            if error is not None:
                raise error
            if future.result():
                report(f"Compiled Xbox shader {key}")
        except ShaderRefused as exc:
            failed = True
            if exc.cached:
                cached_refusals += 1
            else:
                refused += 1
                report(f"Xbox shader {key}: {exc}")
        except Exception as exc:
            # A worker failure must release its guest waiter. Never turn a
            # transient tool or programming error into a persistent refusal.
            failed = True
            report(f"Xbox shader {key}: {exc}")
        finally:
            # Publish the terminal result before releasing the source name.
            # Once it disappears, the guest may clear the old failure and
            # publish a new generation of the same request.
            if failed:
                (session / (key + ".failed")).touch()
            remove_request(source)

    directory = session / "requests"
    active = {}
    report(f"Xbox shader worker: up to {jobs} concurrent requests")
    with ThreadPoolExecutor(max_workers=jobs, thread_name_prefix="xshader") as pool:
        while active or not stop.is_set():
            for key, (source, future) in list(active.items()):
                if future.done():
                    complete(source, key, future)
                    del active[key]
            with archive_lock:
                messages = archive_errors[:]
                archive_errors.clear()
            for message in messages:
                report(message)
            # No unbounded executor queue: on shutdown only running jobs
            # finish, before the session's files and pinned compiler disappear.
            if not stop.is_set() and len(active) < jobs:
                for source in ordered_requests(directory):
                    if stop.is_set() or len(active) >= jobs:
                        break
                    key = source.stem
                    if not re.fullmatch(REQUEST_PATTERNS[source.suffix], key):
                        remove_request(source)
                        continue
                    if key in active:
                        continue
                    if tools is None:
                        try:
                            tools, cache = bind_cache()
                        except (OSError, ValueError, subprocess.SubprocessError) as exc:
                            complete(source, key, error=exc)
                            continue
                    active[key] = (source, pool.submit(compile_one, source, key, tools, cache))
            if active:
                # Wake immediately when any worker completes, with a bounded
                # rescan interval for new requests. Do not spin during stop.
                wait_futures([future for _, future in active.values()],
                             timeout=0.05, return_when=FIRST_COMPLETED)
            elif not stop.is_set():
                wait()
    if refused or cached_refusals:
        report(f"Xbox shader worker: {refused} new unsupported translations, {cached_refusals} cached refusals")


@contextmanager
def runtime_shader_compiler(backend, env, executable, report):
    if backend != "proton" or env.get("TUXBOX_RUNTIME_SHADERS") == "0" or not executable.with_name("d3d12_xs.dll").is_file():
        yield
        return
    cache = Path(env["WINEPREFIX"]) / SOURCE_CACHE.parent
    from .scarlett_prep import scarlett_requests

    archive = None
    try:
        archive = scarlett_requests(executable, cache)
        if archive is not None:
            archive.mkdir(parents=True, mode=0o700, exist_ok=True)
    except OSError as exc:
        report(f"Could not open ProjectScarlett's shader inputs: {exc}")
        archive = None
    with shader_session(cache, env, report, archive=archive):
        yield


@contextmanager
def shader_session(cache, env, report, *, archive=None):
    cache.mkdir(parents=True, exist_ok=True)
    with TemporaryDirectory(prefix="run-", dir=cache) as tmp:
        session = Path(tmp)
        (session / "requests").mkdir()
        env["TUXBOX_SHADER_SESSION"] = session.name
        stop = Event()
        helper = env.get("TUXBOX_PERSISTENT_DXC")
        options = {"dxc_helper": helper} if helper else {}
        worker = Thread(target=serve, args=(session, stop, report, archive, runtime_jobs(env, report)),
                        kwargs=options, name="shader-compiler", daemon=True)
        worker.start()
        try:
            yield
        finally:
            stop.set()
            worker.join()
            env.pop("TUXBOX_SHADER_SESSION", None)
