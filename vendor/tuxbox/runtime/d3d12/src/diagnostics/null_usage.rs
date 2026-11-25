//! Bounded per-report ranking of recorded commands using substituted PSOs.
//! No native pointer identities or strong references to D3D resources.
use std::{collections::HashMap, sync::Arc};

#[derive(Debug, PartialEq, Eq, Hash)]
pub(crate) struct Tag {
    pub(crate) kind: &'static str,
    pub(crate) stages: String,
}

#[derive(Debug, Default, PartialEq, Eq, Clone, Copy)]
struct Counts {
    commands: u64,
    display_bound: u64,
}
impl Counts {
    fn add(&mut self, display: bool) {
        self.commands = self.commands.saturating_add(1);
        self.display_bound = self.display_bound.saturating_add(u64::from(display));
    }
}
pub(crate) struct Usage {
    limit: usize,
    entries: HashMap<Arc<Tag>, Counts>,
    overflow: Counts,
    total: Counts,
    compute: u64,
    raster: u64,
    lifetime: u64,
    interval: u64,
}
impl Default for Usage {
    fn default() -> Self { Self::new(1024) }
}
impl Usage {
    fn new(limit: usize) -> Self {
        Self { limit, entries: HashMap::new(), overflow: Counts::default(), total: Counts::default(), compute: 0, raster: 0, lifetime: 0, interval: 0 }
    }
    pub(crate) fn record(&mut self, tag: &Arc<Tag>, display: bool) -> bool {
        self.lifetime = self.lifetime.saturating_add(1);
        self.total.add(display);
        if tag.kind == "compute" { self.compute = self.compute.saturating_add(1); }
        if tag.kind == "raster" { self.raster = self.raster.saturating_add(1); }
        if let Some(counts) = self.entries.get_mut(tag) {
            counts.add(display);
        } else if self.entries.len() < self.limit {
            let mut counts = Counts::default();
            counts.add(display);
            self.entries.insert(Arc::clone(tag), counts);
        } else {
            self.overflow.add(display);
        }
        // Bounded diagnostics even if the title never presents a frame.
        self.lifetime == 1 || (self.lifetime >= 64 && self.lifetime.is_power_of_two())
    }
    pub(crate) fn drain(&mut self) -> Vec<String> {
        self.interval = self.interval.saturating_add(1);
        let interval = self.interval;
        let mut rows: Vec<_> = self.entries.drain().collect();
        rows.sort_unstable_by(|(a, ac), (b, bc)| {
            bc.display_bound.cmp(&ac.display_bound)
                .then_with(|| bc.commands.cmp(&ac.commands))
                .then_with(|| a.kind.cmp(b.kind))
                .then_with(|| a.stages.cmp(&b.stages))
        });
        let total = std::mem::take(&mut self.total);
        let overflow = std::mem::take(&mut self.overflow);
        let compute = std::mem::take(&mut self.compute);
        let raster = std::mem::take(&mut self.raster);
        let mut result = vec![format!(
            "null usage interval recorded={} display_bound={} tracked={} overflow={} overflow_display_bound={} compute={} raster={} interval={interval}",
            total.commands, total.display_bound, rows.len(), overflow.commands, overflow.display_bound, compute, raster)];
        result.extend(rows.into_iter().take(8).map(|(tag, count)| format!(
            "null usage rank kind={} {} recorded={} display_bound={} interval={interval}",
            tag.kind, tag.stages, count.commands, count.display_bound)));
        result
    }
}

#[cfg(test)]
#[path = "../tests/diagnostics/null_usage.rs"]
mod tests;
