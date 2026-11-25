//! Byte-level directory reader for DirectX shader containers.
//!
//! The [LLVM container format](https://llvm.org/docs/DirectX/DXContainer.html#basic-structure)
//! describes the header, part directory, and length-prefixed parts.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    pub fourcc: [u8; 4],
    /// Offset of the payload from the container's first byte.
    pub offset: usize,
    pub size: usize,
}

#[derive(Debug, Clone)]
pub struct Dxbc {
    pub digest: [u8; 16],
    pub total_size: u32,
    pub chunks: Vec<Chunk>,
}

pub(super) fn take<const N: usize>(bytes: &mut &[u8]) -> Result<[u8; N], &'static str> {
    let (field, rest) = bytes
        .split_first_chunk::<N>()
        .ok_or("truncated DXBC field")?;
    *bytes = rest;
    Ok(*field)
}

pub(super) fn word(bytes: &mut &[u8]) -> Result<u32, &'static str> {
    Ok(u32::from_le_bytes(take(bytes)?))
}

impl Dxbc {
    /// Reads a container at the start of a buffer, retaining the directory order.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid header, incomplete directory, or part
    /// whose header or payload overlaps another part or exceeds the container.
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        let mut header = bytes;
        if take::<4>(&mut header)? != *b"DXBC" {
            return Err("invalid DXBC magic".into());
        }
        let digest = take(&mut header)?;
        let _version = word(&mut header)?;
        let total_size = word(&mut header)?;
        let count = word(&mut header)? as usize;

        const HEADER_SIZE: usize = 32;
        let container = bytes
            .get(..total_size as usize)
            .ok_or("truncated DXBC container")?;
        let directory_end = count
            .checked_mul(4)
            .and_then(|n| HEADER_SIZE.checked_add(n))
            .ok_or("DXBC directory size overflow")?;
        let mut directory = container
            .get(HEADER_SIZE..directory_end)
            .ok_or("truncated DXBC directory")?;
        let mut chunks = Vec::with_capacity(count);
        while !directory.is_empty() {
            let start = word(&mut directory)? as usize;
            if start < directory_end {
                return Err("DXBC part starts inside the header or directory".into());
            }
            let mut part = container
                .get(start..)
                .ok_or("DXBC part offset out of range")?;
            let fourcc = take(&mut part)?;
            let size = word(&mut part)? as usize;
            if size > part.len() {
                return Err("truncated DXBC part payload".into());
            }
            chunks.push(Chunk {
                fourcc,
                offset: start + 8,
                size,
            });
        }

        // Directory entries can refer to parts in a different physical order.
        let mut by_offset: Vec<_> = chunks.iter().collect();
        by_offset.sort_unstable_by_key(|chunk| chunk.offset);
        for pair in by_offset.windows(2) {
            if pair[0].offset + pair[0].size > pair[1].offset - 8 {
                return Err("overlapping DXBC parts".into());
            }
        }
        Ok(Self {
            digest,
            total_size,
            chunks,
        })
    }

    pub fn chunk(&self, tag: &[u8; 4]) -> Option<&Chunk> {
        self.chunks.iter().find(|part| part.fourcc == *tag)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn container_with(parts: &[(&[u8; 4], &[u8])]) -> Vec<u8> {
        let mut file = Vec::from(*b"DXBC");
        file.extend_from_slice(&[0x1d; 16]);
        file.extend_from_slice(&1u32.to_le_bytes());
        file.extend_from_slice(&[0; 4]);
        file.extend_from_slice(&(parts.len() as u32).to_le_bytes());
        file.resize(32 + parts.len() * 4, 0);
        for (index, (tag, payload)) in parts.iter().enumerate() {
            let position = file.len() as u32;
            file[32 + 4 * index..36 + 4 * index].copy_from_slice(&position.to_le_bytes());
            file.extend_from_slice(tag.as_slice());
            file.extend_from_slice(&(payload.len() as u32).to_le_bytes());
            file.extend_from_slice(payload);
        }
        let size = file.len() as u32;
        file[24..28].copy_from_slice(&size.to_le_bytes());
        file
    }

    #[test]
    fn preserves_directory_order_and_respects_embedded_size() {
        let mut file = container_with(&[(b"AAAA", &[8, 3, 1]), (b"BBBB", &[])]);
        let declared = file.len();
        file[32..40].rotate_left(4);
        file.extend_from_slice(b"trailing data");
        let parsed = Dxbc::parse(&file).unwrap();
        assert_eq!(parsed.digest, [0x1d; 16]);
        assert_eq!(parsed.total_size as usize, declared);
        assert_eq!(&parsed.chunks[0].fourcc, b"BBBB");
        let part = parsed.chunk(b"AAAA").unwrap();
        assert_eq!(&file[part.offset..part.offset + part.size], &[8, 3, 1]);
        assert!(parsed.chunk(b"CCCC").is_none());
    }

    #[test]
    fn rejects_directory_and_part_ranges_outside_declared_bounds() {
        let file = container_with(&[(b"TEST", &[0; 8])]);
        for (position, invalid) in [
            (24, 16),
            (24, 51),
            (24, 53),
            (28, u32::MAX),
            (32, 4),
            (32, 34),
            (32, 49),
            (32, u32::MAX),
            (40, u32::MAX),
        ] {
            let mut corrupt = file.clone();
            corrupt[position..position + 4].copy_from_slice(&invalid.to_le_bytes());
            assert!(
                Dxbc::parse(&corrupt).is_err(),
                "field {position} value {invalid}"
            );
        }
        for n in 0..file.len() {
            assert!(Dxbc::parse(&file[..n]).is_err(), "length {n}");
        }
        assert!(Dxbc::parse(&container_with(&[])).unwrap().chunks.is_empty());
    }

    #[test]
    fn detects_duplicate_or_overlapping_part_headers() {
        let file = container_with(&[(b"LEFT", &[0; 8]), (b"RGHT", &[])]);
        for second in [40u32, 48] {
            let mut corrupt = file.clone();
            corrupt[36..40].copy_from_slice(&second.to_le_bytes());
            assert!(Dxbc::parse(&corrupt).is_err());
        }
    }
}
