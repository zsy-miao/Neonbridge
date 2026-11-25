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
#[path = "tests/container.rs"]
pub(crate) mod tests;
