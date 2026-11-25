//! The first root signatures the title creates, decoded from their RTS0 part
//! (re-derived from rebox 73c1b1f). TUXBOX_ROOT_SIG_LOG=N logs the first N
//! (default 16, 0 off): version, flags, each parameter and static samplers.
use std::sync::atomic::{AtomicU32, Ordering};

static SEEN: AtomicU32 = AtomicU32::new(0);

fn limit() -> u32 {
    static LIMIT: std::sync::LazyLock<u32> =
        std::sync::LazyLock::new(|| std::env::var("TUXBOX_ROOT_SIG_LOG").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(16));
    *LIMIT
}

pub(crate) fn note(blob: &[u8]) {
    let n = SEEN.fetch_add(1, Ordering::Relaxed);
    if n >= limit() {
        return;
    }
    let text = decode(blob).unwrap_or_else(|| format!("{} bytes, no readable RTS0 part", blob.len()));
    super::log_line(format_args!("root signature #{}: {text}", n + 1));
}

/// A root signature blob decoded as the `root signature #n` lines show it.
pub(crate) fn describe(blob: &[u8]) -> String {
    decode(blob).unwrap_or_else(|| format!("{} bytes, no readable RTS0 part", blob.len()))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    b.get(at..at + 4).map(|s| u32::from_le_bytes(s.try_into().unwrap()))
}

/// The RTS0 part of a DXBC container, or the blob itself when it is a bare RTS0.
fn rts0(blob: &[u8]) -> Option<&[u8]> {
    if blob.get(0..4) != Some(b"DXBC") {
        return Some(blob);
    }
    let parts = u32_at(blob, 28)? as usize;
    for i in 0..parts.min(64) {
        let offset = u32_at(blob, 32 + 4 * i)? as usize;
        if blob.get(offset..offset + 4) == Some(b"RTS0") {
            let size = u32_at(blob, offset + 4)? as usize;
            return blob.get(offset + 8..offset + 8 + size);
        }
    }
    None
}

fn decode(blob: &[u8]) -> Option<String> {
    let r = rts0(blob)?;
    let version = u32_at(r, 0)?;
    let count = u32_at(r, 4)? as usize;
    let params = u32_at(r, 8)? as usize;
    let samplers = u32_at(r, 12)?;
    let flags = u32_at(r, 20)?;
    let range_size = if version >= 2 { 24 } else { 20 };
    let mut out = Vec::new();
    for i in 0..count.min(64) {
        let at = params + 12 * i;
        let (kind, visibility, offset) = (u32_at(r, at)?, u32_at(r, at + 4)?, u32_at(r, at + 8)? as usize);
        let vis = ["all", "vs", "hs", "ds", "gs", "ps", "as", "ms"].get(visibility as usize).copied().unwrap_or("?");
        let body = match kind {
            0 => {
                let ranges = u32_at(r, offset)? as usize;
                let first = u32_at(r, offset + 4)? as usize;
                // TUXBOX_ROOT_RANGES_LOG (default 64): ranges shown per table, the
                // rest counted. test183's roots stopped at 16 with no mark, so a
                // shader's t81 at table index 29 looked missing from the root.
                let shown = std::env::var("TUXBOX_ROOT_RANGES_LOG").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(64usize);
                let mut list: Vec<String> = (0..ranges.min(shown)).filter_map(|j| {
                    let at = first + range_size * j;
                    let kind = ["srv", "uav", "cbv", "sampler"].get(u32_at(r, at)? as usize).copied().unwrap_or("?");
                    let num = u32_at(r, at + 4)?;
                    let num = if num == u32::MAX { "unbounded".to_string() } else { num.to_string() };
                    Some(format!("{kind} {num} at {}/space{}", u32_at(r, at + 8)?, u32_at(r, at + 12)?))
                }).collect();
                if ranges > shown {
                    list.push(format!("+{} more", ranges - shown));
                }
                format!("table [{}]", list.join(", "))
            }
            1 => format!("constants {} at b{}/space{}", u32_at(r, offset + 8)?, u32_at(r, offset)?, u32_at(r, offset + 4)?),
            2..=4 => format!("{} at {}/space{}", ["cbv", "srv", "uav"][kind as usize - 2], u32_at(r, offset)?, u32_at(r, offset + 4)?),
            _ => format!("kind {kind}"),
        };
        out.push(format!("{i}:{vis} {body}"));
    }
    Some(format!("v1.{} flags {flags:#x} {count} parameters {{{}}} static samplers {samplers}", version.saturating_sub(1), out.join("; ")))
}
