//! Free ranges of the shader-visible heap.
use super::*;

/// Coalescing best-fit free-span allocator over `[0, used)` shader-visible slots.
#[derive(Default)]
pub(super) struct FreeSpans {
    pub(super) by_start: BTreeMap<u32, u32>,
    pub(super) by_len: BTreeSet<(u32, u32)>,
    pub(super) total: u32,
}

impl FreeSpans {
    pub(super) fn insert_span(&mut self, start: u32, len: u32) {
        debug_assert!(len > 0);
        let prev = self.by_start.insert(start, len);
        debug_assert!(prev.is_none());
        let inserted = self.by_len.insert((len, start));
        debug_assert!(inserted);
        self.total += len;
    }

    pub(super) fn remove_span(&mut self, start: u32, len: u32) {
        let prev = self.by_start.remove(&start);
        debug_assert_eq!(prev, Some(len));
        let existed = self.by_len.remove(&(len, start));
        debug_assert!(existed);
        self.total -= len;
    }

    pub(super) fn alloc(&mut self, need: u32) -> Option<u32> {
        if need == 0 {
            return None;
        }
        let &(len, start) = self.by_len.range((need, 0)..).next()?;
        self.remove_span(start, len);
        if len > need {
            self.insert_span(start + need, len - need);
        }
        Some(start)
    }

    pub(super) fn free(&mut self, start: u32, len: u32, used: &mut u32) {
        if len == 0 {
            return;
        }
        let mut new_start = start;
        let mut new_len = len;
        if let Some((&prev_start, &prev_len)) = self.by_start.range(..new_start).next_back()
            && prev_start.checked_add(prev_len) == Some(new_start)
        {
            self.remove_span(prev_start, prev_len);
            new_start = prev_start;
            new_len += prev_len;
        }
        if let Some(end) = new_start.checked_add(new_len)
            && let Some(&next_len) = self.by_start.get(&end)
        {
            self.remove_span(end, next_len);
            new_len += next_len;
        }
        if new_start.checked_add(new_len) == Some(*used) {
            *used = new_start;
        } else {
            self.insert_span(new_start, new_len);
        }
    }

    pub(super) fn free_slots(&self) -> u32 {
        self.total
    }

    pub(super) fn largest_span(&self) -> u32 {
        self.by_len.last().map_or(0, |&(len, _)| len)
    }
}
