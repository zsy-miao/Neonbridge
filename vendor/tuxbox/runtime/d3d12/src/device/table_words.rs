//! The exact guest words of a root-descriptor table a translated shader
//! reads as data (xshader's "descriptors read as data": about 125 programs
//! s_load T#/V# words from a user-data table pointer and shift or mask them;
//! the words are title memory, so nothing is encoded). A stage declaring
//! `ByteAddressBuffer tb_tw{p} : register(t{p}, space32765)` gets one raw
//! root SRV over the title memory at the address the title bound at guest
//! root parameter `p` (a root CBV, SRV or UAV), up to the end of its
//! allocation or 64 KiB, whichever is less, and reads guest dword j at byte
//! off + 4j. An unresolved or unbound parameter reads the null sink. Space
//! 32765 holds only private static samplers otherwise, so its t registers
//! are free.
//!
//! TUXBOX_TABLE_WORDS=0 leaves the bindings out.
pub(crate) const SPACE: u32 = 32765;
/// The longest window bound (the title's table and the T#s after it).
pub(crate) const WINDOW: u64 = 64 << 10;

pub(crate) fn enabled() -> bool {
    crate::env_toggle!("TUXBOX_TABLE_WORDS") != Some("0")
}

/// The guest root parameters whose table words the stage reads (its SRV
/// registers in space 32765), ascending.
pub(crate) fn declared(code: &[u8]) -> Vec<u32> {
    if !enabled() {
        return Vec::new();
    }
    let mut params: Vec<u32> = super::null_kinds::bindings(code).iter()
        .filter(|&&[resource, space, _, _, _]| matches!(resource, 3..=5) && space == SPACE)
        .flat_map(|&[_, _, lower, upper, _]| lower..=upper.min(lower.saturating_add(63)))
        .collect();
    params.sort_unstable();
    params.dedup();
    params
}

/// Merges two stages' parameter lists.
pub(crate) fn union(mut a: Vec<u32>, b: Vec<u32>) -> Vec<u32> {
    a.extend(b);
    a.sort_unstable();
    a.dedup();
    a
}
