//! Opt-in, bounded allocation size diagnostics. No resource data is logged.
use std::sync::{LazyLock, atomic::{AtomicUsize, Ordering}};
use windows::Win32::Graphics::Direct3D12::{D3D12_RESOURCE_ALLOCATION_INFO, D3D12_RESOURCE_DESC};

fn parse_sizes(value: &str) -> Option<Vec<u64>> {
    let mut sizes = Vec::new();
    for item in value.split(',') {
        let item = item.trim();
        let bytes = if let Some(hex) = item.strip_prefix("0x") {
            u64::from_str_radix(hex, 16).ok()?
        } else if item == "invalid" {
            u64::MAX
        } else {
            item.parse().ok()?
        };
        if !sizes.contains(&bytes) {
            if sizes.len() == 16 { return None; }
            sizes.push(bytes);
        }
    }
    Some(sizes)
}

fn sizes() -> &'static [u64] {
    static SIZES: LazyLock<Vec<u64>> = LazyLock::new(|| {
        std::env::var("TUXBOX_TRACE_ALLOCATION_BYTES").ok()
            .and_then(|v| parse_sizes(&v)).unwrap_or_default()
    });
    &SIZES
}

pub(super) fn enabled() -> bool {
    !sizes().is_empty() && crate::diagnostics::logging()
}

pub(super) fn matches(bytes: u64) -> bool {
    enabled() && sizes().contains(&bytes)
}

pub(super) fn report(kind: &str, items: &[D3D12_RESOURCE_DESC], answer: &D3D12_RESOURCE_ALLOCATION_INFO) {
    use crate::diagnostics::{Seen, Sighting};
    static SEEN: Seen<String> = Seen::new(256);
    static NEXT: AtomicUsize = AtomicUsize::new(1);
    if items.is_empty() { return; }
    let caller = crate::survey::title_rva().unwrap_or(0);
    // Cap per-batch detail, but show the true count. Avoid copying resource
    // contents or pointers; the log is useful across ASLR and driver builds.
    let header = format!("allocation trace {kind} caller={caller:#x} count={} bytes={} align={}",
        items.len(), answer.SizeInBytes, answer.Alignment);
    let details: Vec<_> = items.iter().take(8).enumerate().map(|(idx, d)| {
        format!(
            "item{idx}:dim={} {}x{}x{} mips={} format={} samples={}:{} layout={:#x} flags={:#x} align={}",
            d.Dimension.0, d.Width, d.Height, d.DepthOrArraySize, d.MipLevels,
            d.Format.0, d.SampleDesc.Count, d.SampleDesc.Quality, d.Layout.0, d.Flags.0, d.Alignment)
    }).collect();
    let key = format!("{header} {}", details.join(" "));
    match SEEN.sight(key.as_str()) {
        Sighting::New => {
            // Keep individual lines short for Wine's debug output and attach
            // a common ID because separate title threads may interleave them.
            let id = NEXT.fetch_add(1, Ordering::Relaxed);
            crate::diagnostics::log(&format!("{header} id={id}"));
            for detail in details {
                crate::diagnostics::log(&format!("allocation trace id={id} {detail}"));
            }
        }
        Sighting::Full => crate::diagnostics::log("allocation trace capped at 256 distinct records"),
        Sighting::Repeat => {}
    }
}

#[cfg(test)]
mod tests {
    use super::parse_sizes;

    #[test]
    fn parses_exact_decimal_hex_and_invalid_sizes() {
        assert_eq!(parse_sizes("8192, 0x4000, invalid, 8192"), Some(vec![8192, 16384, u64::MAX]));
        for bad in ["", "0x", "-1", "8,", "8192,garbage", "18446744073709551616"] {
            assert_eq!(parse_sizes(bad), None, "{bad}");
        }
        let too_many = (0..17).map(|n| n.to_string()).collect::<Vec<_>>().join(",");
        assert_eq!(parse_sizes(&too_many), None);
    }
}
