//! Track only descriptors with additional guest state, such as placed SRVs and stencil DSVs.
use super::lock;
use std::{
    collections::{BTreeMap, VecDeque},
    ops::Range,
    sync::{Arc, Mutex, Weak},
    time::{Duration, Instant},
};
use windows::Win32::Graphics::Direct3D12::*;

pub(crate) type Views<T> = DescriptorMap<Weak<T>>;
pub(crate) type HeapViews<T> = DescriptorRange<Weak<T>>;

/// The second field keeps values replaced in a descriptor slot alive for
/// RETIRE_GRACE (TUXBOX_DESCRIPTOR_RETIRE, default on, =0 drops them at the
/// write as before). A descriptor write is immediate on the CPU, but lists
/// that already reference the old descriptor may still run on the GPU; the
/// old view's backing (an imported buffer, placed texture or shadow copy)
/// was released under the running dispatch: an SM MMU fault (Xid 13).
/// From TuxBox e4783b4 (PR #132, GPL-3.0, credit to its authors).
pub(crate) struct DescriptorMap<V>(Mutex<BTreeMap<usize, V>>, Mutex<VecDeque<(Instant, V)>>);

const RETIRE_GRACE: Duration = Duration::from_secs(3);

fn retire_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| crate::env_toggle!("TUXBOX_DESCRIPTOR_RETIRE") != Some("0"))
}

impl<V> Default for DescriptorMap<V> {
    fn default() -> Self {
        Self(Default::default(), Default::default())
    }
}

impl<V> DescriptorMap<V> {
    /// Holds replaced values until in-flight GPU work is done with them.
    fn retire(&self, replaced: impl IntoIterator<Item = V>) {
        if !retire_on() {
            drop(replaced.into_iter().collect::<Vec<_>>());
            return;
        }
        let now = Instant::now();
        let expired: Vec<(Instant, V)> = {
            let mut queue = lock(&self.1);
            queue.extend(replaced.into_iter().map(|v| (now, v)));
            let keep = queue.partition_point(|(at, _)| now.duration_since(*at) >= RETIRE_GRACE);
            queue.drain(..keep).collect()
        };
        // Dropped outside the lock: a backing's Drop takes other registries' locks.
        drop(expired);
    }
}

pub(crate) struct DescriptorRange<V> {
    views: Arc<DescriptorMap<V>>,
    pub(crate) range: Range<usize>,
}

impl<V: Clone> DescriptorRange<V> {
    pub(crate) fn copy_to(&self, stride: usize, dst: usize, src: usize, count: u32) {
        self.views.copy(stride, std::iter::once((dst, count)), std::iter::once((src, count)));
    }

    /// Whether `handle` holds a tracked view (TUXBOX_HEAP_TEXTURE_QUEUE).
    pub(crate) fn has(&self, handle: usize) -> bool {
        lock(&self.views.0).contains_key(&handle)
    }
}

impl DescriptorRange<PlacedView> {
    /// The handles in `handles` whose placed texture's title memory meets
    /// `[lo, hi)` (TUXBOX_HEAP_TEXTURE_QUEUE's remap hook).
    pub(crate) fn handles_over(&self, handles: Range<usize>, lo: u64, hi: u64) -> Vec<usize> {
        lock(&self.views.0).range(handles)
            .filter(|(_, v)| v.texture.upgrade().is_some_and(|t| { let r = t.range(); r.start < hi && lo < r.end }))
            .map(|(&h, _)| h).collect()
    }
}

impl<V> Drop for DescriptorRange<V> {
    fn drop(&mut self) {
        // The heap is gone: no clamp record may point into it (pages::unclamp).
        super::pages::views_gone(self.range.clone());
        lock(&self.views.0)
            .extract_if(self.range.clone(), |_, _| true)
            .for_each(drop);
    }
}

impl<T> Views<T> {
    pub(crate) fn get(&self, handle: usize) -> Option<Arc<T>> {
        lock(&self.0).get(&handle)?.upgrade()
    }

    pub(crate) fn set(&self, handle: usize, value: Option<&Arc<T>>) {
        self.store(handle, value.map(Arc::downgrade));
    }

    /// Calls `f` with each live value whose handle lies in one of `ranges`.
    pub(crate) fn each_in(&self, ranges: &[Range<usize>], mut f: impl FnMut(&Arc<T>)) {
        let views = lock(&self.0);
        if views.is_empty() { return; }
        for range in ranges {
            for (_, view) in views.range(range.clone()) {
                if let Some(value) = view.upgrade() { f(&value); }
            }
        }
    }

    pub(crate) fn preceding(&self, address: usize) -> Option<Arc<T>> {
        lock(&self.0).range(..=address).next_back()?.1.upgrade()
    }

    pub(crate) fn remove_if(&self, handle: usize, value: *const T) {
        let mut entries = lock(&self.0);
        if entries.get(&handle).is_some_and(|entry| entry.as_ptr() == value) {
            entries.remove(&handle);
        }
    }

}

#[derive(Clone)]
pub(crate) struct PlacedView {
    texture: Weak<super::Texture>,
    writable: bool,
}

impl DescriptorMap<PlacedView> {
    pub(crate) fn set(&self, handle: usize, texture: Option<&Arc<super::Texture>>, writable: bool) {
        super::pages::view_written(handle);
        if texture.is_some_and(|t| super::is_plane_desc(&t.desc)) {
            super::PLANE_VIEWS_SEEN.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        self.store(handle, texture.map(|t| PlacedView { texture: Arc::downgrade(t), writable }));
    }

    /// Every live plane texture (R8/R8G8, 256 wide or more) with a view,
    /// by handle, for the plane gather log.
    pub(crate) fn plane_views(&self) -> Vec<(usize, Arc<super::Texture>)> {
        lock(&self.0).iter()
            .filter_map(|(&h, v)| v.texture.upgrade().filter(|t| super::is_plane_desc(&t.desc)).map(|t| (h, t)))
            .collect()
    }

    /// `written`: the view ranges the lists' programs write (their UAV
    /// tables, heap writes and whole-heap fallbacks); None counts every
    /// writable view as written.
    pub(crate) fn gather(&self, heaps: &[Range<usize>], written: Option<&[Range<usize>]>, reads: &mut Vec<Arc<super::Texture>>, writes: &mut Vec<Arc<super::Texture>>) {
        if heaps.is_empty() { return; }
        let views = lock(&self.0);
        for heap in heaps {
            for (&handle, view) in views.range(heap.clone()) {
                if let Some(texture) = view.texture.upgrade() {
                    let write = view.writable && written.is_none_or(|w| w.iter().any(|r| r.contains(&handle)));
                    if write { writes.push(texture); } else { reads.push(texture); }
                }
            }
        }
    }
}
impl<V> DescriptorMap<V> {
    /// Calls `f` with each value whose handle lies in one of `ranges`.
    pub(crate) fn each_value_in(&self, ranges: &[Range<usize>], mut f: impl FnMut(&V)) {
        let views = lock(&self.0);
        if views.is_empty() { return; }
        for range in ranges {
            for (_, view) in views.range(range.clone()) {
                f(view);
            }
        }
    }
}

impl<V: Clone> DescriptorMap<V> {
    pub(crate) fn lookup(&self, handle: usize) -> Option<V> {
        lock(&self.0).get(&handle).cloned()
    }

    /// Drops every entry in `range`.
    pub(crate) fn remove_range(&self, range: Range<usize>) {
        let gone: Vec<V> = lock(&self.0).extract_if(range, |_, _| true).map(|(_, v)| v).collect();
        drop(gone);
    }

    /// The (handle, value) entries `keep` picks.
    pub(crate) fn matching(&self, keep: impl Fn(&V) -> bool) -> Vec<(usize, V)> {
        lock(&self.0).iter().filter(|(_, v)| keep(v)).map(|(&h, v)| (h, v.clone())).collect()
    }

    pub(crate) fn store(&self, handle: usize, value: Option<V>) {
        let old = {
            let mut views = lock(&self.0);
            if let Some(value) = value {
                views.insert(handle, value)
            } else {
                views.remove(&handle)
            }
        };
        if old.is_some() { self.retire(old); }
    }

    pub(crate) fn heap(
        self: &Arc<Self>,
        device: &ID3D12Device,
        native: &ID3D12DescriptorHeap,
        desc: &D3D12_DESCRIPTOR_HEAP_DESC,
        kind: D3D12_DESCRIPTOR_HEAP_TYPE,
    ) -> Option<DescriptorRange<V>> {
        if desc.Type != kind {
            return None;
        }
        let start = unsafe { native.GetCPUDescriptorHandleForHeapStart() }.ptr;
        let stride = unsafe { device.GetDescriptorHandleIncrementSize(desc.Type) } as usize;
        Some(DescriptorRange {
            views: self.clone(),
            range: start..start + stride * desc.NumDescriptors as usize,
        })
    }

    pub(crate) fn copy(
        &self,
        stride: usize,
        destinations: impl Iterator<Item = (usize, u32)>,
        sources: impl Iterator<Item = (usize, u32)>,
    ) {
        let mut replaced = Vec::new();
        let mut views = lock(&self.0);
        if views.is_empty() {
            return;
        }
        // The source and destination descriptor counts match, but their range
        // boundaries need not. Null size arrays default to one slot per range.
        let mut dst = destinations.filter(|(_, count)| *count > 0);
        let mut src = sources.filter(|(_, count)| *count > 0);
        let mut d = dst.next();
        let mut s = src.next();
        let mut copied = Vec::new();
        while let (Some((to, left)), Some((from, available))) = (d, s) {
            let count = left.min(available);
            let span = count as usize * stride;
            copied.extend(
                views
                    .range(from..from + span)
                    .map(|(&key, value)| (to + (key - from), value.clone())),
            );
            replaced.extend(views.extract_if(to..to + span, |_, _| true).map(|(_, v)| v));
            views.extend(copied.drain(..));
            d = if count == left {
                dst.next()
            } else {
                Some((to + span, left - count))
            };
            s = if count == available {
                src.next()
            } else {
                Some((from + span, available - count))
            };
        }
        drop(views);
        if !replaced.is_empty() { self.retire(replaced); }
    }
}
