//! Bounded weak lookup of exact live page imports. Ownership remains with resources.
use std::{collections::{HashMap, VecDeque}, sync::{Arc, Weak}};

const LIMIT: usize = 4096;
const SWEEP: usize = 16;
const PAGE: u64 = 65536;

/// The key of an import spanning several whole pages at `base`. A page-aligned
/// base has its low 16 bits clear, so they carry the page count: two imports
/// share a key only when they cover exactly the same pages. A single page
/// keeps its base as the key, which no span key can equal.
pub(super) fn span_key(base: u64, size: u64) -> Option<u64> {
    let pages = size / PAGE;
    (base.is_multiple_of(PAGE) && size.is_multiple_of(PAGE) && (2..PAGE).contains(&pages)).then_some(base | pages)
}

pub(super) struct PageCache<T> {
    entries: HashMap<u64, Weak<T>>,
    sweep: VecDeque<u64>,
}

impl<T> Default for PageCache<T> {
    fn default() -> Self { Self { entries: HashMap::new(), sweep: VecDeque::new() } }
}

impl<T> PageCache<T> {
    /// Reuse the live import at `base` while `usable` accepts it. A rejected
    /// entry (for example, one whose pages were since remapped) is replaced.
    pub(super) fn get_or_open<E>(&mut self, base: u64, usable: impl Fn(&T) -> bool, open: impl FnOnce() -> Result<Arc<T>, E>) -> Result<Arc<T>, E> {
        if let Some(value) = self.entries.get(&base).and_then(Weak::upgrade).filter(|value| usable(value)) {
            return Ok(value);
        }
        // Do not retain an unsuccessful open or any strong ownership.
        let value = open()?;
        if let Some(entry) = self.entries.get_mut(&base) {
            // A remapped key must be replaceable even at capacity. Keep
            // exactly one position in the sweep queue for each key.
            *entry = Arc::downgrade(&value);
            return Ok(value);
        }
        if self.entries.len() >= LIMIT {
            // Incremental reclamation: avoid walking 4096 live weak entries
            // for every cache miss while the guest retains all of its pages.
            for _ in 0..SWEEP.min(self.sweep.len()) {
                let Some(key) = self.sweep.pop_front() else { break };
                if self.entries.get(&key).is_some_and(|entry| entry.strong_count() != 0) {
                    self.sweep.push_back(key);
                } else {
                    self.entries.remove(&key);
                }
            }
        }
        if self.entries.len() < LIMIT {
            self.entries.insert(base, Arc::downgrade(&value));
            self.sweep.push_back(base);
        }
        Ok(value)
    }
}

#[cfg(test)]
#[path = "../tests/device/heap_page_cache.rs"]
mod tests;
