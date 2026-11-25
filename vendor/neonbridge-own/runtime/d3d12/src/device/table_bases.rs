//! TUXBOX_TABLE_BASES (default on, =0 off): the host heap index of the first
//! slot of each guest descriptor-table root parameter a translated stage
//! indexes at a run-time offset. xshader reads a table descriptor loaded at a
//! dynamic SOFFSET (`s_load [table], s<n>` into a table of many ranges) as
//! `ResourceDescriptorHeap[tb_tbase<p> + ((imm + s<n>) >> 5)]`, declaring
//! `cbuffer tb_tbc<p> : register(b<p>, space32765) { uint tb_tbase<p>; }`;
//! each such parameter gets one root constant here, set at bind time from the
//! table's GPU handle in the heap the draw binds. Copied from ztrx
//! victory-shader emit/lower.rs heap_index (GPL-3.0, credit to its authors):
//! the table pointer is a heap address, the load's offset and SOFFSET added
//! to it, over 32 the descriptor's heap index. Victory passes the table's
//! heap address as user data; ours passes its first slot's index.
pub(crate) const SPACE: u32 = tuxbox_dxbc::reserved::TABLE_BASES.space;

pub(crate) fn enabled() -> bool {
    crate::env_toggle!("TUXBOX_TABLE_BASES") != Some("0")
}

/// The guest root parameters whose table base the stage reads (its CBV
/// registers in space 32765), ascending.
pub(crate) fn declared(code: &[u8]) -> Vec<u32> {
    if !enabled() {
        return Vec::new();
    }
    let mut params: Vec<u32> = super::null_kinds::bindings(code).iter()
        .filter(|&&[resource, space, _, _, _]| resource == 2 && space == SPACE)
        .flat_map(|&[_, _, lower, upper, _]| lower..=upper.min(lower.saturating_add(63)))
        .collect();
    params.sort_unstable();
    params.dedup();
    params
}
