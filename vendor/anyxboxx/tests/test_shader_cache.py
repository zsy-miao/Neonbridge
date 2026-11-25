"""Cache disque des shaders compilés (emu/shader_cache.py) et son usage par rdna2_gpu.compile_dxil."""
import time

import pytest

from emu import rdna2_gpu, rdna2_hlsl, shader_cache
from test_rdna2_exec import ONE, program, vop1

DXIL = b"DXBC" + bytes(60)


def test_round_trip_and_key_inputs(tmp_path):
    cache = shader_cache.ShaderCache(tmp_path)
    key = shader_cache.cache_key("hlsl", "cs_6_0", None)
    assert cache.get(key) is None
    cache.put(key, DXIL)
    assert cache.get(key) == DXIL and (cache.stats.hits, cache.stats.misses) == (1, 1)
    assert key != shader_cache.cache_key("hlsl ", "cs_6_0", None)
    assert key != shader_cache.cache_key("hlsl", "cs_6_6", None)


def test_compiler_update_changes_the_key(tmp_path):
    compiler = tmp_path / "dxc.exe"
    compiler.write_bytes(b"v1")
    before = shader_cache.cache_key("hlsl", "cs_6_0", compiler)
    compiler.write_bytes(b"version 2")
    assert shader_cache.cache_key("hlsl", "cs_6_0", compiler) != before


def test_corrupted_entries_are_dropped_and_non_dxil_refused(tmp_path):
    cache = shader_cache.ShaderCache(tmp_path)
    key = shader_cache.cache_key("x", "cs_6_0", None)
    cache.put(key, DXIL)
    path = next(tmp_path.glob("*/*.dxil"))
    path.write_bytes(b"garbage")
    fresh = shader_cache.ShaderCache(tmp_path)                    # autre processus : pas de couche mémoire
    assert fresh.get(key) is None and not path.exists()
    with pytest.raises(ValueError):
        cache.put(key, b"not dxil")


def test_oldest_entries_are_pruned(tmp_path):
    cache = shader_cache.ShaderCache(tmp_path, limit_bytes=3 * len(DXIL))
    keys = [shader_cache.cache_key(str(i), "cs_6_0", None) for i in range(5)]
    for key in keys:
        cache.put(key, DXIL)
        time.sleep(0.01)
    assert len(cache.entries()) == 3
    fresh = shader_cache.ShaderCache(tmp_path)
    assert fresh.get(keys[0]) is None and fresh.get(keys[-1]) == DXIL


@pytest.mark.skipif(rdna2_gpu.find_dxc() is None, reason="dxc absent")
def test_second_compilation_comes_from_the_cache(tmp_path, monkeypatch):
    cache = shader_cache.ShaderCache(tmp_path)
    hlsl = rdna2_hlsl.translate(program(vop1("v_mov_b32", 1, ONE)))
    started = time.perf_counter()
    first = rdna2_gpu.compile_dxil(hlsl, cache)
    compile_time = time.perf_counter() - started
    dxc = rdna2_gpu.find_dxc()
    monkeypatch.setattr(rdna2_gpu, "compile_hlsl", lambda *_: pytest.fail("dxc rappelé malgré le cache"))
    monkeypatch.setattr(rdna2_gpu, "find_dxc", lambda: dxc)
    started = time.perf_counter()
    assert rdna2_gpu.compile_dxil(hlsl, cache) == first
    assert time.perf_counter() - started < compile_time
    assert first.startswith(b"DXBC") and cache.stats.hits == 1
