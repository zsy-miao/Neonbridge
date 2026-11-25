//! Binary adapter for exercising the standalone registry with GPU fixtures.
mod registry;
use std::io::{self, Read, Write};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut input = Vec::new();
    io::stdin().read_to_end(&mut input)?;
    if !input.len().is_multiple_of(40) || input.len() / 40 > 3 {
        return Err("invalid mapping records".into());
    }
    let mappings: Vec<_> = input
        .chunks_exact(40)
        .map(|r| {
            let wide = |at| u64::from_le_bytes(r[at..at + 8].try_into().unwrap());
            let word = |at| u32::from_le_bytes(r[at..at + 4].try_into().unwrap());
            let allocation = wide(16);
            registry::Mapping {
                guest_start: wide(0),
                bytes: word(8),
                slot: word(12),
                allocation: (allocation != u64::MAX).then_some(allocation),
                allocation_offset: wide(24),
                view_bytes: wide(32),
            }
        })
        .collect();
    let entries = registry::encode(&mappings).map_err(|e| format!("{e:?}"))?;
    let mut out = io::stdout().lock();
    for entry in entries {
        for word in entry {
            out.write_all(&word.to_le_bytes())?;
        }
    }
    Ok(())
}
