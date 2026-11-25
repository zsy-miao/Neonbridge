//! Title-memory reads by guest address through the runtime's page directory
//! (TUXBOX_MEM_DIR). Ported from MESIA-Unified crates/mesia-shader/src/emit/lower.rs
//! `memory()` (:2632-2695) and crates/mesia-gpu/src/memory.rs (GPL-3.0, also in ztrx and
//! VItoriaBr), with credit: one 8-byte {copy, start} entry per 64 KiB page of the 40-bit
//! address space; copy 0 is absent (the page is queued in the fault list and the read
//! gives zeros); otherwise the bytes are in raw view `tb_mem_base + copy` of the resource
//! heap at `start + (address & 0xffff)`. The runtime recognises a program by its t0/space32764 binding (PSV0). TB_MEM_DIR=0 keeps the no-hit stand-ins.

pub(super) fn on() -> bool {
    std::env::var("TB_MEM_DIR").map_or(true, |v| v.trim() != "0")
}

/// Vertex, geometry and pixel stages read through the directory too (the runtime binds
/// it for graphics pipelines, TUXBOX_MEM_DIR_GRAPHICS). TB_MEM_DIR_GRAPHICS=0 refuses.
pub(super) fn graphics(stage: super::Stage) -> bool {
    matches!(stage, super::Stage::Vertex | super::Stage::Geometry | super::Stage::Pixel)
        && std::env::var("TB_MEM_DIR_GRAPHICS").map_or(true, |v| v.trim() != "0")
}

pub(super) const DECLARATIONS: &str = r#"
ByteAddressBuffer tb_mem_dir : register(t0, space32764);
RWByteAddressBuffer tb_mem_faults : register(u0, space32764);
cbuffer tb_mem : register(b0, space32764) { uint tb_mem_base; };
// One dword of title memory at the 40-bit address (lo, hi); dword aligned.
uint tb_mem_load(uint lo, uint hi) {
    uint page = min((lo >> 16) | ((hi & 0xffu) << 16), 0x1fffffu);
    uint2 e = tb_mem_dir.Load2(page * 8u);
    // Faulting lanes record the page at ((page % 4095) + 1) * 4; the rest store to the sink at 0.
    tb_mem_faults.Store(e.x == 0u ? ((page % 4095u) + 1u) * 4u : 0u, page);
    if (e.x == 0u) return 0u;
    RWByteAddressBuffer copy = ResourceDescriptorHeap[NonUniformResourceIndex(tb_mem_base + e.x)];
    return copy.Load((e.y + (lo & 0xffffu)) & ~3u);
}
"#;
