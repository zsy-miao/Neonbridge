//! Track only descriptors with additional guest state, such as placed SRVs and stencil DSVs.
use super::lock;
use std::{
    collections::BTreeMap,
    ops::Range,
    sync::{Arc, Mutex, Weak},
};
use windows::Win32::Graphics::Direct3D12::*;

pub(crate) type Views<T> = DescriptorMap<Weak<T>>;
pub(crate) type HeapViews<T> = DescriptorRange<Weak<T>>;

pub(crate) struct DescriptorMap<V>(Mutex<BTreeMap<usize, V>>);

impl<V> Default for DescriptorMap<V> {
    fn default() -> Self {
        Self(Default::default())
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
        let mut views = lock(&self.0);
        if let Some(value) = value {
            views.insert(handle, value);
        } else {
            views.remove(&handle);
        }
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
            views.extract_if(to..to + span, |_, _| true).for_each(drop);
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
    }
}
