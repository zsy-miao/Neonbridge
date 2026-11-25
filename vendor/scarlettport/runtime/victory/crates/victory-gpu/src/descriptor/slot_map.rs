//! Title view slots past a capped host heap's identity window, each given a
//! host slot of the heap's pool while it holds a view (see
//! `host::HostHeap`). NVIDIA takes 1,000,000 shader-visible views; the
//! title's view heap has 1,245,184 slots, 500,000 to 700,000 of
//! them in use, spread over the whole heap. Title slots below the window
//! keep their own index on the host; the others are mapped, and translated
//! shaders look the host slot up in the heap map (a buffer of [`entry`]
//! values, one per title slot from the window on) before they index the
//! heap.
//!
//! Pure bookkeeping: the caller writes the descriptors and the map buffer.
//! A host slot is never moved while its title slot holds a view. A slot
//! given back waits in quarantine until every submission that may have read
//! it through the old map entry has finished: the caller passes the number
//! of submissions started when it gives a slot back and the number finished
//! when it asks for one.

use std::collections::VecDeque;

/// The map buffer's entry for `host`: its bitwise not, 0 for none. A zeroed
/// buffer, and a read past its end (which returns 0), then give `u32::MAX`,
/// which translated shaders clamp to the heap's guard slot.
pub(crate) fn entry(host: Option<u32>) -> u32 {
    host.map_or(0, |h| !h)
}

/// The host slot an [`entry`] names, `None` for 0.
pub(crate) fn decode(entry: u32) -> Option<u32> {
    (entry != 0).then_some(!entry)
}

/// Title slots from `window` to `window + entries.len()` mapped onto the
/// host slots `first..first + pool`.
#[derive(Debug)]
pub(crate) struct SlotMap {
    window: u32,
    first: u32,
    pool: u32,
    /// Per title slot from the window on: its host slot as an [`entry`].
    entries: Vec<u32>,
    /// Pool slots never handed out start at `first + fresh`.
    fresh: u32,
    /// Pool slots given back that no submission can still read.
    free: Vec<u32>,
    /// Pool slots given back, with the submissions started at the time,
    /// oldest first.
    quarantine: VecDeque<(u64, u32)>,
    /// Title slots refused a host slot because the pool was used up.
    pub(crate) refused: u64,
}

/// What [`SlotMap::stats`] counts.
#[cfg(any(test, feature = "diagnostics"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Stats {
    pub(crate) mapped: u32,
    pub(crate) free: u32,
    pub(crate) quarantined: u32,
    pub(crate) refused: u64,
}

impl SlotMap {
    /// A map of title slots `window..title_count` onto `pool` host slots
    /// from `first`.
    pub(crate) fn new(window: u32, title_count: u32, first: u32, pool: u32) -> Self {
        SlotMap {
            window,
            first,
            pool,
            entries: vec![0; title_count.saturating_sub(window) as usize],
            fresh: 0,
            free: Vec::new(),
            quarantine: VecDeque::new(),
            refused: 0,
        }
    }

    /// The first title slot mapped.
    pub(crate) fn window(&self) -> u32 {
        self.window
    }

    /// The entries, as the map buffer holds them.
    pub(crate) fn len(&self) -> u32 {
        self.entries.len() as u32
    }

    /// The host slot of title slot `title`: its own index below the window,
    /// else the one mapped to it, if any.
    pub(crate) fn host(&self, title: u32) -> Option<u32> {
        match title.checked_sub(self.window) {
            None => Some(title),
            Some(at) => decode(*self.entries.get(at as usize)?),
        }
    }

    /// The map buffer's entry for title slot `title` (at index
    /// `title - window`).
    pub(crate) fn entry(&self, title: u32) -> u32 {
        title
            .checked_sub(self.window)
            .and_then(|at| self.entries.get(at as usize))
            .copied()
            .unwrap_or(0)
    }

    /// The host slot of title slot `title`, taking a pool slot for a mapped
    /// one that has none. `finished`: the submissions finished so far, which
    /// releases the quarantined slots no running submission can read.
    /// `None` past the title's slots, or when the pool is used up.
    pub(crate) fn assign(&mut self, title: u32, finished: u64) -> Option<u32> {
        let Some(at) = title.checked_sub(self.window) else {
            return Some(title);
        };
        let held = *self.entries.get(at as usize)?;
        if let Some(host) = decode(held) {
            return Some(host);
        }
        while let Some(&(started, host)) = self.quarantine.front()
            && started <= finished
        {
            self.quarantine.pop_front();
            self.free.push(host);
        }
        let host = match self.free.pop() {
            Some(host) => host,
            None if self.fresh < self.pool => {
                self.fresh += 1;
                self.first + self.fresh - 1
            }
            None => {
                self.refused += 1;
                return None;
            }
        };
        self.entries[at as usize] = entry(Some(host));
        Some(host)
    }

    /// Gives back the host slot of mapped title slot `title`, if it has
    /// one: quarantined until the submissions `started` so far finish.
    pub(crate) fn free(&mut self, title: u32, started: u64) -> Option<u32> {
        let at = title.checked_sub(self.window)?;
        let slot = self.entries.get_mut(at as usize)?;
        let host = decode(*slot)?;
        *slot = 0;
        self.quarantine.push_back((started, host));
        Some(host)
    }

    /// The title slots that hold a host slot, with it, in title order.
    pub(crate) fn mapped(&self) -> impl Iterator<Item = (u32, u32)> + '_ {
        self.entries
            .iter()
            .enumerate()
            .filter_map(|(at, &e)| Some((self.window + at as u32, decode(e)?)))
    }

    /// Mapped title slots, free and quarantined pool slots, refusals.
    #[cfg(any(test, feature = "diagnostics"))]
    pub(crate) fn stats(&self) -> Stats {
        let held = self.fresh as usize - self.free.len() - self.quarantine.len();
        Stats {
            mapped: held as u32,
            free: self.pool - self.fresh + self.free.len() as u32,
            quarantined: self.quarantine.len() as u32,
            refused: self.refused,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Title slots below the window are their own host slot; the others
    /// take pool slots in order, keep them, and none past the title's.
    #[test]
    fn title_slots_past_the_window_take_pool_slots_and_keep_them() {
        let mut m = SlotMap::new(16, 64, 20, 8);
        assert_eq!(m.len(), 48);
        assert_eq!(m.host(3), Some(3));
        assert_eq!(m.assign(3, 0), Some(3));
        assert_eq!(m.host(40), None);
        assert_eq!(m.assign(40, 0), Some(20));
        assert_eq!(m.assign(17, 0), Some(21));
        assert_eq!(
            m.assign(40, 0),
            Some(20),
            "a mapped slot keeps its host slot"
        );
        assert_eq!(m.host(40), Some(20));
        assert_eq!(m.host(63), None);
        assert_eq!(m.assign(64, 0), None, "past the title's slots");
        assert_eq!(m.free(3, 0), None, "slots below the window are not mapped");
        assert_eq!(
            m.stats(),
            Stats {
                mapped: 2,
                free: 6,
                quarantined: 0,
                refused: 0
            }
        );
    }

    /// The buffer holds each host slot's bitwise not; 0 (never written, or
    /// read past the end) is no slot, and reads as `u32::MAX`.
    #[test]
    fn entries_hold_the_host_slot_inverted() {
        assert_eq!(entry(None), 0);
        assert_eq!(entry(Some(131_072)), !131_072);
        assert_eq!(decode(entry(Some(0))), Some(0));
        assert_eq!(decode(entry(Some(999_997))), Some(999_997));
        assert_eq!(decode(0), None);
        assert_eq!(!0u32, u32::MAX, "what a shader makes of an empty entry");
        let mut m = SlotMap::new(16, 64, 20, 8);
        assert_eq!(m.entry(40), 0);
        m.assign(40, 0);
        assert_eq!(m.entry(40), !20);
        assert_eq!(m.entry(3), 0, "below the window: no entry");
        m.free(40, 0);
        assert_eq!(m.entry(40), 0);
    }

    /// A slot given back is not handed out while a submission started
    /// before it was given back may still run.
    #[test]
    fn freed_slots_wait_for_the_submissions_running_when_freed() {
        let mut m = SlotMap::new(0, 8, 100, 2);
        assert_eq!(m.assign(0, 0), Some(100));
        assert_eq!(m.assign(1, 0), Some(101));
        // Submission 5 had started (4 finished) when slot 0 emptied.
        assert_eq!(m.free(0, 5), Some(100));
        assert_eq!(m.free(0, 5), None, "given back once");
        assert_eq!(m.stats().quarantined, 1);
        assert_eq!(m.assign(2, 4), None, "submission 5 may still read it");
        assert_eq!(m.refused, 1);
        assert_eq!(m.assign(2, 5), Some(100), "submission 5 finished");
        assert_eq!(m.stats().quarantined, 0);
        // Given back in order, released in order.
        assert_eq!(m.free(1, 6), Some(101));
        assert_eq!(m.free(2, 7), Some(100));
        assert_eq!(m.assign(3, 6), Some(101));
        assert_eq!(m.assign(4, 6), None);
        assert_eq!(m.assign(4, 7), Some(100));
    }

    /// With every pool slot held, new title slots are refused (they read
    /// the guard), and the held ones are not moved.
    #[test]
    fn a_used_up_pool_refuses_and_moves_nothing() {
        let mut m = SlotMap::new(4, 12, 4, 3);
        let held: Vec<_> = (4..7).map(|t| m.assign(t, 0)).collect();
        assert_eq!(held, [Some(4), Some(5), Some(6)]);
        assert_eq!(m.assign(7, u64::MAX), None);
        assert_eq!(m.assign(8, u64::MAX), None);
        assert_eq!(m.refused, 2);
        assert_eq!((4..7).map(|t| m.host(t)).collect::<Vec<_>>(), held);
        assert_eq!(m.entry(7), 0);
        assert_eq!(m.stats().free, 0);
        m.free(5, 0);
        assert_eq!(m.assign(7, 0), Some(5));
        assert_eq!(m.host(4), Some(4));
        assert_eq!(m.host(6), Some(6));
    }

    /// The mapped title slots are listed with their host slots, in title
    /// order; a slot given back is not.
    #[test]
    fn mapped_lists_the_title_slots_holding_host_slots() {
        let mut m = SlotMap::new(16, 64, 20, 8);
        m.assign(40, 0);
        m.assign(17, 0);
        m.assign(63, 0);
        assert_eq!(m.mapped().collect::<Vec<_>>(), [(17, 21), (40, 20), (63, 22)]);
        m.free(40, 0);
        assert_eq!(m.mapped().collect::<Vec<_>>(), [(17, 21), (63, 22)]);
    }

    /// An empty pool (a window that leaves no room) maps nothing.
    #[test]
    fn an_empty_pool_maps_nothing() {
        let mut m = SlotMap::new(10, 20, 10, 0);
        assert_eq!(m.assign(9, 0), Some(9));
        assert_eq!(m.assign(10, 0), None);
        assert_eq!(m.entry(10), 0);
    }
}
