//! Per-op recording and preparing logic that needs no GPU, behind the
//! `VICTORY_PREP_*` switches of `victory-gpu`: which held barriers can be
//! recorded as one call (and which of them are no-ops), which descriptor
//! table sets repeat what the list already has, and which pending refreshes
//! a title copy must wait for. Kept here so it is tested on every host.

/// D3D12 resource states that only read (`D3D12_RESOURCE_STATE_*`): vertex
/// and constant buffer, index buffer, depth read, non-pixel and pixel shader
/// resource, indirect argument, copy source, resolve source, shading rate
/// source. `COMMON` (0) is not among them: it decays and promotes.
pub const READ_ONLY_STATES: u32 = 0x1 | 0x2 | 0x20 | 0x40 | 0x80 | 0x200 | 0x800 | 0x2000 | 0x0100_0000;

/// Whether `state` is a non-empty combination of read-only states.
pub const fn read_only(state: u32) -> bool {
    state != 0 && state & !READ_ONLY_STATES == 0
}

/// One held barrier, as the coalescing sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Held {
    /// A transition of `resource`'s `subresource` (`flags`: split-barrier
    /// bits; only whole barriers, `flags == 0`, are folded).
    Transition { resource: usize, subresource: u32, before: u32, after: u32, flags: u32 },
    /// A wait for unordered access to `resource` (0: all resources).
    Uav { resource: usize },
    /// Anything else (aliasing): kept, and nothing folds across it.
    Other,
}

/// What [`coalesce`] keeps of a run of held barriers recorded with no work
/// between them: indices into the input, in order, each with the `before`
/// state a folded transition starts from (`None`: as it was).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Coalesced {
    pub keep: Vec<(usize, Option<u32>)>,
    /// Transitions whose state does not change (left out).
    pub noops: u32,
    /// Waits for unordered access another wait in the run already covers.
    pub uavs_dropped: u32,
    /// Pairs `A -> B`, `B -> C` of one subresource made one `A -> C` (`B`
    /// read-only, `A != C`).
    pub folded: u32,
}

/// Coalesces `held` (barriers with no work recorded between them) into
/// what one `ResourceBarrier` call needs, in order:
/// - a transition whose before and after are equal is left out;
/// - a wait for unordered access is left out when an earlier one in the run
///   covers it (one on all resources, or one on the same resource), unless
///   an aliasing barrier lies between;
/// - `A -> B` then `B -> C` of the same subresource become `A -> C` at the
///   later one's place when `B` only reads and `A != C` (no work ran in `B`,
///   and `A -> C` makes `A`'s writes visible as `A -> B` did). `A == C` is
///   kept as two transitions: the pair still orders writes of `A`.
///
/// Nothing is reordered relative to barriers of the same resource.
pub fn coalesce(held: &[Held]) -> Coalesced {
    let mut out = Coalesced::default();
    // Per kept entry: its input and its current `before` (folds rewrite it).
    let mut kept: Vec<(usize, Option<u32>)> = Vec::with_capacity(held.len());
    let mut all_uav = false;
    let mut uavs: Vec<usize> = Vec::new();
    for (i, h) in held.iter().enumerate() {
        match *h {
            Held::Transition { before, after, .. } if before == after => out.noops += 1,
            Held::Transition { resource, subresource, before, after, flags } => {
                // The last kept transition of this subresource, if nothing
                // other than barriers of other resources lies between.
                let last = kept.iter().rposition(|&(k, _)| match held[k] {
                    Held::Transition { resource: r, .. } => r == resource,
                    Held::Other => true,
                    Held::Uav { resource: r } => r == resource || r == 0,
                });
                let fold = last.and_then(|at| {
                    let (k, b) = kept[at];
                    match held[k] {
                        Held::Transition { resource: r, subresource: s, before: first, after: middle, flags: f }
                            if r == resource && s == subresource && f == 0 && flags == 0 && middle == before =>
                        {
                            let first = b.unwrap_or(first);
                            (read_only(middle) && first != after).then_some((at, first))
                        }
                        _ => None,
                    }
                });
                match fold {
                    Some((at, first)) => {
                        kept.remove(at);
                        kept.push((i, Some(first)));
                        out.folded += 1;
                    }
                    None => kept.push((i, None)),
                }
            }
            Held::Uav { resource } => {
                if all_uav || (resource != 0 && uavs.contains(&resource)) {
                    out.uavs_dropped += 1;
                } else {
                    if resource == 0 {
                        all_uav = true;
                    } else {
                        uavs.push(resource);
                    }
                    kept.push((i, None));
                }
            }
            Held::Other => {
                all_uav = false;
                uavs.clear();
                kept.push((i, None));
            }
        }
    }
    out.keep = kept;
    out
}

/// The descriptor tables a list has set per root parameter, for skipping a
/// set of the same table again. Valid only for the root signature binding
/// that was current when they were set: [`TableCache::root_set`] forgets
/// them (setting a root signature on a list invalidates its root
/// arguments; so does a change of descriptor heaps, which sets the root
/// signature again).
#[derive(Clone, Debug)]
pub struct TableCache {
    handles: [u64; 64],
    valid: u64,
}

impl Default for TableCache {
    fn default() -> Self {
        Self::new()
    }
}

impl TableCache {
    pub const fn new() -> Self {
        Self { handles: [0; 64], valid: 0 }
    }

    /// A root signature was set (or may have been): nothing is known set.
    pub fn root_set(&mut self) {
        self.valid = 0;
    }

    /// Whether a set of `handle` at `param` must be recorded (and notes it
    /// set). Parameters past 63 are always recorded.
    pub fn set(&mut self, param: u32, handle: u64) -> bool {
        let Some(bit) = 1u64.checked_shl(param) else {
            return true;
        };
        if self.valid & bit != 0 && self.handles[param as usize] == handle {
            return false;
        }
        self.valid |= bit;
        self.handles[param as usize] = handle;
        true
    }
}

/// Whether `[a0, a1)` and `[b0, b1)` share a page of `page` bytes (empty
/// ranges share none). A title copy into one range must first finish
/// pending refreshes of the other when they do: refreshes take whole
/// written pages.
pub fn pages_meet((a0, a1): (usize, usize), (b0, b1): (usize, usize), page: usize) -> bool {
    if a0 >= a1 || b0 >= b1 || page == 0 {
        return a0 < a1 && b0 < b1 && a0 < b1 && b0 < a1;
    }
    let (a0, a1) = (a0 / page, a1.div_ceil(page));
    let (b0, b1) = (b0 / page, b1.div_ceil(page));
    a0 < b1 && b0 < a1
}

#[cfg(test)]
mod tests {
    use super::*;

    const RT: u32 = 0x4;
    const UAV: u32 = 0x8;
    const SRV: u32 = 0x40 | 0x80;
    const COPY_DEST: u32 = 0x400;
    const COPY_SRC: u32 = 0x800;

    fn t(resource: usize, before: u32, after: u32) -> Held {
        Held::Transition { resource, subresource: u32::MAX, before, after, flags: 0 }
    }

    #[test]
    fn read_only_states() {
        assert!(read_only(SRV));
        assert!(read_only(COPY_SRC | 0x20));
        assert!(!read_only(0));
        assert!(!read_only(RT));
        assert!(!read_only(SRV | UAV));
    }

    #[test]
    fn independent_barriers_all_kept_in_order() {
        let held = [t(1, RT, SRV), t(2, UAV, SRV), Held::Uav { resource: 3 }, t(4, COPY_DEST, SRV)];
        let c = coalesce(&held);
        assert_eq!(c.keep, vec![(0, None), (1, None), (2, None), (3, None)]);
        assert_eq!((c.noops, c.uavs_dropped, c.folded), (0, 0, 0));
    }

    #[test]
    fn noop_transition_dropped() {
        let c = coalesce(&[t(1, SRV, SRV), t(2, RT, SRV)]);
        assert_eq!(c.keep, vec![(1, None)]);
        assert_eq!(c.noops, 1);
    }

    #[test]
    fn repeated_uav_waits_dropped() {
        let held = [
            Held::Uav { resource: 0 },
            Held::Uav { resource: 0 },
            Held::Uav { resource: 5 },
            t(1, RT, SRV),
            Held::Uav { resource: 0 },
        ];
        let c = coalesce(&held);
        assert_eq!(c.keep, vec![(0, None), (3, None)]);
        assert_eq!(c.uavs_dropped, 3);
    }

    #[test]
    fn same_resource_uav_waits_dropped_but_not_others() {
        let held = [Held::Uav { resource: 5 }, Held::Uav { resource: 6 }, Held::Uav { resource: 5 }];
        let c = coalesce(&held);
        assert_eq!(c.keep, vec![(0, None), (1, None)]);
        assert_eq!(c.uavs_dropped, 1);
    }

    #[test]
    fn aliasing_resets_uav_dedupe() {
        let held = [Held::Uav { resource: 0 }, Held::Other, Held::Uav { resource: 0 }];
        let c = coalesce(&held);
        assert_eq!(c.keep, vec![(0, None), (1, None), (2, None)]);
        assert_eq!(c.uavs_dropped, 0);
    }

    #[test]
    fn fold_through_read_state() {
        // RT -> SRV, other resource, SRV -> COPY_DEST: one RT -> COPY_DEST.
        let held = [t(1, RT, SRV), t(2, UAV, SRV), t(1, SRV, COPY_DEST)];
        let c = coalesce(&held);
        assert_eq!(c.keep, vec![(1, None), (2, Some(RT))]);
        assert_eq!(c.folded, 1);
    }

    #[test]
    fn fold_chains() {
        let held = [t(1, UAV, SRV), t(1, SRV, COPY_SRC), t(1, COPY_SRC, RT)];
        let c = coalesce(&held);
        assert_eq!(c.keep, vec![(2, Some(UAV))]);
        assert_eq!(c.folded, 2);
    }

    #[test]
    fn no_fold_back_to_start() {
        // RT -> SRV -> RT still orders the render target writes: kept.
        let held = [t(1, RT, SRV), t(1, SRV, RT)];
        let c = coalesce(&held);
        assert_eq!(c.keep, vec![(0, None), (1, None)]);
        assert_eq!(c.folded, 0);
    }

    #[test]
    fn no_fold_through_write_state() {
        let held = [t(1, SRV, COPY_DEST), t(1, COPY_DEST, RT)];
        let c = coalesce(&held);
        assert_eq!(c.keep.len(), 2);
        assert_eq!(c.folded, 0);
    }

    #[test]
    fn no_fold_other_subresource_or_split() {
        let a = Held::Transition { resource: 1, subresource: 0, before: RT, after: SRV, flags: 0 };
        let b = Held::Transition { resource: 1, subresource: 1, before: SRV, after: COPY_DEST, flags: 0 };
        assert_eq!(coalesce(&[a, b]).folded, 0);
        let split = Held::Transition { resource: 1, subresource: 0, before: SRV, after: COPY_DEST, flags: 2 };
        assert_eq!(coalesce(&[a, split]).folded, 0);
    }

    #[test]
    fn no_fold_across_uav_wait_on_it_or_aliasing() {
        let held = [t(1, RT, SRV), Held::Uav { resource: 1 }, t(1, SRV, COPY_DEST)];
        assert_eq!(coalesce(&held).folded, 0);
        let held = [t(1, RT, SRV), Held::Uav { resource: 0 }, t(1, SRV, COPY_DEST)];
        assert_eq!(coalesce(&held).folded, 0);
        let held = [t(1, RT, SRV), Held::Other, t(1, SRV, COPY_DEST)];
        assert_eq!(coalesce(&held).folded, 0);
    }

    #[test]
    fn mismatched_chain_not_folded() {
        // The second does not start where the first ended.
        let held = [t(1, RT, SRV), t(1, COPY_SRC, COPY_DEST)];
        assert_eq!(coalesce(&held).folded, 0);
    }

    #[test]
    fn empty_run() {
        assert_eq!(coalesce(&[]), Coalesced::default());
    }

    #[test]
    fn table_cache_skips_repeats_until_root_set() {
        let mut c = TableCache::new();
        assert!(c.set(2, 100));
        assert!(!c.set(2, 100));
        assert!(c.set(2, 101));
        assert!(!c.set(2, 101));
        assert!(c.set(3, 101));
        c.root_set();
        assert!(c.set(2, 101));
        assert!(c.set(3, 101));
        // A handle of 0 is a handle like any other once set.
        assert!(c.set(0, 0));
        assert!(!c.set(0, 0));
        // Out of range: always recorded.
        assert!(c.set(64, 1));
        assert!(c.set(64, 1));
        assert!(c.set(63, 7));
        assert!(!c.set(63, 7));
    }

    #[test]
    fn pages_meet_by_page() {
        const P: usize = 4096;
        assert!(pages_meet((0, 10), (4000, 4010), P));
        assert!(!pages_meet((0, 4096), (4096, 5000), P));
        assert!(pages_meet((0, 4097), (8000, 8001), P));
        assert!(!pages_meet((0, 10), (10, 10), P));
        assert!(!pages_meet((5, 5), (0, 100), P));
        assert!(pages_meet((8192, 8193), (0, 8193), P));
        assert!(!pages_meet((8192, 8193), (0, 8192), P));
        // Byte ranges when no page size is given.
        assert!(!pages_meet((0, 10), (10, 20), 0));
        assert!(pages_meet((0, 11), (10, 20), 0));
    }
}
