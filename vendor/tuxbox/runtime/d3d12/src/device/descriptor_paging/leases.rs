//! Pins that keep tables resident while command lists use them.
use super::*;

/// Deduplicated pins held by one recording command list until `Reset` or `Drop`.
#[derive(Default)]
pub(crate) struct DescriptorLeases {
    pub(super) arenas: Vec<(Arc<Arena>, HashSet<u64>)>,
}

impl Drop for DescriptorLeases {
    fn drop(&mut self) {
        self.clear();
    }
}

impl DescriptorLeases {
    pub(super) fn find_arena(&self, arena: &Arc<Arena>) -> Option<usize> {
        self.arenas.iter().position(|(known, _)| Arc::ptr_eq(known, arena))
    }

    pub(super) fn contains(&self, arena: &Arc<Arena>, id: u64) -> bool {
        self.find_arena(arena)
            .is_some_and(|index| self.arenas[index].1.contains(&id))
    }

    pub(super) fn reserve_slot(&mut self, arena: &Arc<Arena>) -> Result<()> {
        let index = if let Some(index) = self.find_arena(arena) {
            index
        } else {
            self.arenas.try_reserve(1).map_err(|_| E_OUTOFMEMORY)?;
            self.arenas.push((Arc::clone(arena), HashSet::new()));
            self.arenas.len() - 1
        };
        self.arenas[index]
            .1
            .try_reserve(1)
            .map_err(|_| E_OUTOFMEMORY)?;
        Ok(())
    }

    pub(super) fn insert_reserved(&mut self, arena: &Arc<Arena>, id: u64) {
        let index = self
            .find_arena(arena)
            .expect("arena slot reserved before insert");
        let inserted = self.arenas[index].1.insert(id);
        debug_assert!(inserted);
    }

    pub(crate) fn extend_from(&mut self, other: &Self) -> std::result::Result<(), windows::core::HRESULT> {
        for (arena, ids) in &other.arenas {
            if ids.is_empty() {
                continue;
            }
            let index = if let Some(index) = self.find_arena(arena) {
                index
            } else {
                self.arenas.try_reserve(1).map_err(|_| E_OUTOFMEMORY)?;
                self.arenas.push((Arc::clone(arena), HashSet::new()));
                self.arenas.len() - 1
            };
            let target = &mut self.arenas[index].1;
            let mut to_pin = Vec::new();
            for &id in ids {
                if !target.contains(&id) {
                    to_pin.try_reserve(1).map_err(|_| E_OUTOFMEMORY)?;
                    to_pin.push(id);
                }
            }
            if to_pin.is_empty() {
                continue;
            }
            target.try_reserve(to_pin.len()).map_err(|_| E_OUTOFMEMORY)?;
            arena.pin_ids(&to_pin);
            target.extend(to_pin);
        }
        Ok(())
    }

    pub(crate) fn clear(&mut self) {
        for (arena, ids) in self.arenas.drain(..) {
            if !ids.is_empty() {
                arena.unpin_ids(ids);
            }
        }
    }
}

/// RAII pin guard for an `ExecuteCommandLists` batch prior to GPU commit.
/// Dropping before `commit` or `disarm` unpins every table in the batch.
pub(crate) struct PendingSubmissions {
    pub(super) entries: Vec<(Arc<Arena>, HashSet<u64>)>,
    pub(super) armed: bool,
}

impl Default for PendingSubmissions {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for PendingSubmissions {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        for (arena, ids) in self.entries.drain(..) {
            if !ids.is_empty() {
                arena.unpin_ids(ids);
            }
        }
    }
}

impl PendingSubmissions {
    pub(crate) fn new() -> Self {
        Self {
            entries: Vec::new(),
            armed: true,
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.entries.iter().all(|(_, ids)| ids.is_empty())
    }

    pub(crate) fn pin_from(&mut self, leases: &DescriptorLeases) -> Result<()> {
        for (arena, lease_ids) in &leases.arenas {
            if lease_ids.is_empty() {
                continue;
            }
            let pos = if let Some(pos) = self.entries.iter().position(|(a, _)| Arc::ptr_eq(a, arena)) {
                pos
            } else {
                self.entries.try_reserve(1).map_err(|_| E_OUTOFMEMORY)?;
                self.entries.push((Arc::clone(arena), HashSet::new()));
                self.entries.len() - 1
            };
            let (_, pinned) = &mut self.entries[pos];
            let mut to_pin = Vec::new();
            for &id in lease_ids {
                if !pinned.contains(&id) {
                    to_pin.try_reserve(1).map_err(|_| E_OUTOFMEMORY)?;
                    to_pin.push(id);
                }
            }
            if to_pin.is_empty() {
                continue;
            }
            pinned.try_reserve(to_pin.len()).map_err(|_| E_OUTOFMEMORY)?;
            arena.pin_ids(&to_pin);
            pinned.extend(to_pin);
        }
        Ok(())
    }

    pub(crate) fn disarm(&mut self) {
        self.armed = false;
    }

    pub(crate) fn commit(mut self, fence: RetireFence, value: u64) {
        self.armed = false;
        for (arena, ids) in self.entries.drain(..) {
            if !ids.is_empty() {
                arena.push_in_flight(fence.clone(), value, ids.into_iter().collect());
            }
        }
    }
}
