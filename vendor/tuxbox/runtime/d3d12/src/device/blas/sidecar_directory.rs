//! Checked layout shared with the experimental procedural traversal shader.
//! Encoding a directory does not publish it to a GPU queue or establish AS
//! lifetime/provenance; the runtime must establish those separately.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct InstanceRange {
    pub(crate) first: u32,
    pub(crate) count: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Candidate {
    pub(crate) leaf_offset: u32,
    pub(crate) side: u32,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Error {
    TooLarge,
    InvalidInstanceRange,
    InvalidLeafRange,
    UnsupportedLeaf,
}

/// Encode the bounded version-1 directory. Instance slots correspond to the
/// original TLAS instance array, including disabled instances. Candidate IDs
/// are local to a sidecar BLAS and address the associated instance range.
/// The caller owns the leaf bytes and must keep this exact generation alive.
pub(crate) fn encode(
    leaves: &[u8],
    instances: &[InstanceRange],
    candidates: &[Candidate],
) -> Result<Vec<u8>, Error> {
    let leaf_len = u32::try_from(leaves.len()).map_err(|_| Error::TooLarge)?;
    let instance_count = u32::try_from(instances.len()).map_err(|_| Error::TooLarge)?;
    let candidate_count = u32::try_from(candidates.len()).map_err(|_| Error::TooLarge)?;
    let records = instances
        .len()
        .checked_add(candidates.len())
        .ok_or(Error::TooLarge)?;
    let len = records
        .checked_add(1)
        .and_then(|v| v.checked_mul(16))
        .ok_or(Error::TooLarge)?;
    if len > u32::MAX as usize {
        return Err(Error::TooLarge);
    }
    for range in instances {
        if range
            .first
            .checked_add(range.count)
            .is_none_or(|end| end > candidate_count)
        {
            return Err(Error::InvalidInstanceRange);
        }
    }
    for candidate in candidates {
        let offset = candidate.leaf_offset as usize;
        if !offset.is_multiple_of(64) || offset.checked_add(64).is_none_or(|end| end > leaves.len())
        {
            return Err(Error::InvalidLeafRange);
        }
        let leaf = &leaves[offset..offset + 64];
        let word = |at: usize| u32::from_le_bytes(leaf[at..at + 4].try_into().unwrap());
        let descriptor = word(48);
        let selectors = word(60);
        let paired = descriptor & 0x80 != 0;
        let side = candidate.side;
        if side > 1
            || descriptor & 0x7f != 0
            || selectors >> 16 != 0
            || paired != (word(56) != u32::MAX)
            || paired != (selectors >> 8 != 0)
            || (side == 1 && !paired)
            || !matches!((selectors >> (8 * side)) & 0xff, 2 | 4 | 9)
        {
            return Err(Error::UnsupportedLeaf);
        }
    }
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(len).map_err(|_| Error::TooLarge)?;
    let mut record = |words: [u32; 4]| {
        for word in words {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
    };
    record([1, instance_count, candidate_count, leaf_len]);
    for range in instances {
        record([range.first, range.count, 0, 0]);
    }
    for candidate in candidates {
        record([candidate.leaf_offset, candidate.side, 0, 0]);
    }
    Ok(bytes)
}

#[cfg(test)]
#[path = "../../tests/device/blas/sidecar_directory.rs"]
mod tests;
