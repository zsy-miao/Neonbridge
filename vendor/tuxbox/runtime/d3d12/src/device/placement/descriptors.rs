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
/// [`RETIRE_GRACE`]. A descriptor write is immediate on the CPU timeline, but
/// command lists that already reference the old descriptor may still be
/// running on the GPU. On the console the old view names guest memory that
/// stays mapped; here its backing (an imported buffer or placed texture) would
/// be released under the running dispatch, an SM MMU fault (Xid 13).
pub(crate) struct DescriptorMap<V>(Mutex<BTreeMap<usize, V>>, Mutex<VecDeque<(Instant, V)>>);

const RETIRE_GRACE: Duration = Duration::from_secs(3);

impl<V> Default for DescriptorMap<V> {
    fn default() -> Self {
        Self(Default::default(), Default::default())
    }
}

impl<V> DescriptorMap<V> {
    /// Hold replaced values until in-flight GPU work has finished with them.
    fn retire(&self, replaced: impl IntoIterator<Item = V>) {
        // Dropped outside the lock: a backing's Drop takes other registries' locks.
        drop(self.retire_deferred(replaced));
    }

    fn retire_deferred(&self, replaced: impl IntoIterator<Item = V>) -> Vec<V> {
        let now = Instant::now();
        let mut queue = lock(&self.1);
        queue.extend(replaced.into_iter().map(|v| (now, v)));
        let keep = queue.partition_point(|(at, _)| now.duration_since(*at) >= RETIRE_GRACE);
        queue.drain(..keep).map(|(_, v)| v).collect()
    }

    /// Remove host view tracking for an evicted visible descriptor span,
    /// returning any expired retired entries for the caller to drop outside external locks.
    pub(crate) fn clear_deferred(&self, stride: usize, dst: usize, count: u32) -> Vec<V> {
        if count == 0 || stride == 0 {
            return Vec::new();
        }
        let Some(end) = (count as usize)
            .checked_mul(stride)
            .and_then(|span| dst.checked_add(span))
        else {
            return Vec::new();
        };
        let removed: Vec<V> = {
            let mut views = lock(&self.0);
            if views.is_empty() {
                return Vec::new();
            }
            views
                .extract_if(dst..end, |_, _| true)
                .map(|(_, v)| v)
                .collect()
        };
        if removed.is_empty() {
            Vec::new()
        } else {
            self.retire_deferred(removed)
        }
    }
}

pub(crate) struct DescriptorRange<V> {
    views: Arc<DescriptorMap<V>>,
    pub(crate) range: Range<usize>,
}

impl<V> DescriptorRange<V> {
    #[allow(dead_code)]
    pub(crate) fn clear(&self, stride: usize, dst: usize, count: u32) {
        drop(self.clear_deferred(stride, dst, count));
    }

    pub(crate) fn clear_deferred(&self, stride: usize, dst: usize, count: u32) -> Vec<V> {
        self.views.clear_deferred(stride, dst, count)
    }
}

impl<V: Clone> DescriptorRange<V> {
    pub(crate) fn copy_to_deferred(&self, stride: usize, dst: usize, src: usize, count: u32) -> Vec<V> {
        self.views.copy_deferred(stride, std::iter::once((dst, count)), std::iter::once((src, count)))
    }
}

impl<V> Drop for DescriptorRange<V> {
    fn drop(&mut self) {
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
        self.store(handle, texture.map(|t| PlacedView { texture: Arc::downgrade(t), writable }));
    }

    pub(crate) fn gather(&self, heaps: &[Range<usize>], reads: &mut Vec<Arc<super::Texture>>, writes: &mut Vec<Arc<super::Texture>>) {
        if heaps.is_empty() { return; }
        let views = lock(&self.0);
        for heap in heaps {
            for (_, view) in views.range(heap.clone()) {
                if let Some(texture) = view.texture.upgrade() {
                    if view.writable { writes.push(texture); } else { reads.push(texture); }
                }
            }
        }
    }
}
impl<V: Clone> DescriptorMap<V> {
    pub(crate) fn lookup(&self, handle: usize) -> Option<V> {
        lock(&self.0).get(&handle).cloned()
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
        drop(self.copy_deferred(stride, destinations, sources));
    }

    pub(crate) fn copy_deferred(
        &self,
        stride: usize,
        destinations: impl Iterator<Item = (usize, u32)>,
        sources: impl Iterator<Item = (usize, u32)>,
    ) -> Vec<V> {
        let mut replaced = Vec::new();
        let mut views = lock(&self.0);
        if views.is_empty() {
            return Vec::new();
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
        if replaced.is_empty() {
            Vec::new()
        } else {
            self.retire_deferred(replaced)
        }
    }
}
