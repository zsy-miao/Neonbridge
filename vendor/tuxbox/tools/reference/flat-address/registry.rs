//! Conservative host-side mapping validation for the standalone GPU resolver.
//!
//! `allocation` must identify the actual backing allocation/heap, not merely
//! a resource object. A caller that cannot prove allocation identity or view
//! extent must reject the mapping. This is not connected to runtime resources.
#[derive(Clone, Copy, Debug)]
pub struct Mapping {
    pub guest_start: u64,
    pub bytes: u32,
    pub slot: u32,
    pub allocation: Option<u64>,
    pub allocation_offset: u64,
    pub view_bytes: u64,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    Capacity,
    Invalid,
    MissingProvenance,
    DuplicateSlot,
    GuestOverlap,
    PhysicalOverlap,
}

/// Returns the exact four-word entries expected by the GPU reference shader.
/// The caller must keep every validated allocation alive, immutable and in a
/// legal read state through all submitted GPU work.
pub fn encode(mappings: &[Mapping]) -> Result<Vec<[u32; 4]>, Error> {
    if mappings.len() > 3 {
        return Err(Error::Capacity);
    }
    let mut output = Vec::with_capacity(mappings.len());
    for (i, item) in mappings.iter().enumerate() {
        let allocation = item.allocation.ok_or(Error::MissingProvenance)?;
        let end = item
            .guest_start
            .checked_add(u64::from(item.bytes))
            .ok_or(Error::Invalid)?;
        let physical_end = item
            .allocation_offset
            .checked_add(u64::from(item.bytes))
            .ok_or(Error::Invalid)?;
        if item.guest_start == 0
            || item.guest_start % 4 != 0
            || item.bytes == 0
            || item.bytes % 4 != 0
            || item.slot >= 3
            || item.allocation_offset % 4 != 0
            || item.view_bytes < u64::from(item.bytes)
        {
            return Err(Error::Invalid);
        }
        for previous in &mappings[..i] {
            if item.slot == previous.slot {
                return Err(Error::DuplicateSlot);
            }
            // Earlier entries have already passed these checked additions.
            let other_end = previous
                .guest_start
                .checked_add(u64::from(previous.bytes))
                .ok_or(Error::Invalid)?;
            if item.guest_start < other_end && previous.guest_start < end {
                return Err(Error::GuestOverlap);
            }
            let other_allocation = previous.allocation.ok_or(Error::MissingProvenance)?;
            let other_physical_end = previous
                .allocation_offset
                .checked_add(u64::from(previous.bytes))
                .ok_or(Error::Invalid)?;
            if allocation == other_allocation
                && item.allocation_offset < other_physical_end
                && previous.allocation_offset < physical_end
            {
                return Err(Error::PhysicalOverlap);
            }
        }
        output.push([
            item.guest_start as u32,
            (item.guest_start >> 32) as u32,
            item.bytes,
            item.slot,
        ]);
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn mapping(guest_start: u64, slot: u32, allocation: u64, allocation_offset: u64) -> Mapping {
        Mapping {
            guest_start,
            bytes: 64,
            slot,
            allocation: Some(allocation),
            allocation_offset,
            view_bytes: 64,
        }
    }
    #[test]
    fn encodes_full_width_addresses_and_adjacent_ranges() {
        let entries = [
            mapping(0x1000, 0, 7, 0),
            mapping(0x1040, 1, 7, 64),
            mapping(0xffff_ffff_ffff_f000, 2, 9, 0),
        ];
        assert_eq!(
            encode(&entries).unwrap(),
            [
                [0x1000, 0, 64, 0],
                [0x1040, 0, 64, 1],
                [0xffff_f000, 0xffff_ffff, 64, 2]
            ]
        );
    }
    #[test]
    fn rejects_unknown_or_conflicting_provenance() {
        let a = mapping(0x1000, 0, 7, 0);
        let mut b = mapping(0x2000, 1, 8, 0);
        b.allocation = None;
        assert_eq!(encode(&[a, b]), Err(Error::MissingProvenance));
        b.allocation = Some(7);
        b.allocation_offset = 32;
        assert_eq!(encode(&[a, b]), Err(Error::PhysicalOverlap));
        b.allocation = Some(8);
        b.guest_start = 0x1020;
        assert_eq!(encode(&[a, b]), Err(Error::GuestOverlap));
        b.guest_start = 0x2000;
        b.slot = 0;
        assert_eq!(encode(&[a, b]), Err(Error::DuplicateSlot));
    }
    #[test]
    fn rejects_overflow_alignment_short_views_and_excess_entries() {
        let a = mapping(0x1000, 0, 7, 0);
        for bad in [
            Mapping {
                guest_start: 0,
                ..a
            },
            Mapping {
                guest_start: 0x1001,
                ..a
            },
            Mapping { bytes: 0, ..a },
            Mapping { bytes: 3, ..a },
            Mapping {
                guest_start: u64::MAX - 3,
                ..a
            },
            Mapping {
                allocation_offset: u64::MAX - 3,
                ..a
            },
            Mapping {
                allocation_offset: 1,
                ..a
            },
            Mapping {
                view_bytes: 63,
                ..a
            },
            Mapping { slot: 3, ..a },
        ] {
            assert_eq!(encode(&[bad]), Err(Error::Invalid));
        }
        assert_eq!(encode(&[a, a, a, a]), Err(Error::Capacity));
        assert_eq!(encode(&[]).unwrap().len(), 0);
    }
}
