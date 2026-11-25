//! `SGD5`: the engine's shader file. Each record holds one shader container,
//! deflate-packed. The header tables are not decoded yet, so records are
//! found by their marker and cross-checked by their sizes:
//!
//! ```text
//! u8 name length (with NUL), name, NUL, u8 kind,
//! u32 n, u32 0x000DDCE7, u32 n, u32 unpacked size, raw deflate (n - 12 bytes)
//! ```

pub const MAGIC: &[u8; 4] = b"SGD5";
const MARKER: [u8; 4] = 0x000D_DCE7_u32.to_le_bytes();
/// Containers are at most a few MiB; refuse to allocate for absurd sizes.
const MAX_UNPACKED: usize = 64 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Record<'a> {
    /// Offset of the marker in the file.
    pub offset: usize,
    /// Like `PS_<name>_<hash>_Wrapped`; `None` if the bytes before the
    /// record do not read as one.
    pub name: Option<&'a str>,
    /// Record type, meaning unknown (0x32 most shaders, 0x41 mesh shaders
    /// and some material families, 0x3c compute, 0x40, 0x42).
    pub kind: u8,
    packed: &'a [u8],
    unpacked_size: usize,
}

impl Record<'_> {
    /// The container, or `None` if the stream is corrupt.
    pub fn unpack(&self) -> Option<Vec<u8>> {
        let mut state = zlib_rs::Inflate::new(false, 15);
        let mut out = Vec::new();
        let (mut in_pos, mut out_pos) = (0usize, 0usize);
        loop {
            if out_pos == self.unpacked_size {
                let mut extra = [0u8; 1];
                let status = state
                    .decompress(
                        &self.packed[in_pos..],
                        &mut extra,
                        zlib_rs::InflateFlush::NoFlush,
                    )
                    .ok()?;
                return (status == zlib_rs::Status::StreamEnd
                    && state.total_out() as usize == self.unpacked_size)
                    .then_some(out);
            }
            if out_pos == out.len() {
                let next_len = if out.is_empty() {
                    self.unpacked_size
                        .min(self.packed.len().saturating_mul(4).max(4096))
                } else {
                    out.len().saturating_mul(2).min(self.unpacked_size)
                };
                out.resize(next_len, 0);
            }
            let status = state
                .decompress(
                    &self.packed[in_pos..],
                    &mut out[out_pos..],
                    zlib_rs::InflateFlush::NoFlush,
                )
                .ok()?;
            let next_in = state.total_in() as usize;
            let next_out = state.total_out() as usize;
            match status {
                zlib_rs::Status::StreamEnd => {
                    out.truncate(next_out);
                    return (next_out == self.unpacked_size).then_some(out);
                }
                zlib_rs::Status::Ok if next_in != in_pos || next_out != out_pos => {
                    in_pos = next_in;
                    out_pos = next_out;
                }
                zlib_rs::Status::Ok | zlib_rs::Status::BufError => return None,
            }
        }
    }
}

/// An SGD5 file, or a file holding one (an archive: records are found by
/// their marker, so no index is needed).
pub fn is_sgd5(file: &[u8]) -> bool {
    memchr::memmem::find(file, MAGIC).is_some()
}

/// Every record in an SGD5 file, in file order.
pub fn records(file: &[u8]) -> impl Iterator<Item = Record<'_>> {
    memchr::memmem::find_iter(file, &MARKER).filter_map(|at| record(file, at))
}

fn record(file: &[u8], at: usize) -> Option<Record<'_>> {
    let u32_at = |o: usize| {
        file.get(o..o + 4)
            .map(|b| u32::from_le_bytes(b.try_into().expect("4 bytes")) as usize)
    };
    let n = u32_at(at.checked_sub(4)?)?;
    let unpacked_size = u32_at(at + 8)?;
    if u32_at(at + 4)? != n || n < 12 || unpacked_size > MAX_UNPACKED {
        return None;
    }
    let nul = at.checked_sub(6)?;
    Some(Record {
        offset: at,
        name: (file[nul] == 0).then(|| name_before(file, nul)).flatten(),
        kind: file[at - 5],
        packed: file.get(at + 12..at + n)?,
        unpacked_size,
    })
}

/// The printable name that ends at `nul` and follows its length byte.
fn name_before(file: &[u8], nul: usize) -> Option<&str> {
    let run = file[..nul]
        .iter()
        .rposition(|b| !b.is_ascii_graphic())
        .map_or(0, |p| p + 1);
    // From 32 characters on, the length byte is printable too.
    (run.saturating_sub(1)..nul)
        .find(|&l| usize::from(file[l]) == nul - l)
        .and_then(|l| std::str::from_utf8(&file[l + 1..nul]).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record_bytes(name: &str, kind: u8, payload: &[u8]) -> Vec<u8> {
        let mut buf = vec![0u8; zlib_rs::compress_bound(payload.len())];
        let config = zlib_rs::DeflateConfig {
            window_bits: -15,
            ..zlib_rs::DeflateConfig::new(6)
        };
        let (packed, rc) = zlib_rs::compress_slice(&mut buf, payload, config);
        assert_eq!(rc, zlib_rs::ReturnCode::Ok);
        let n = (packed.len() + 12) as u32;
        let mut out = vec![0xF4, 0x01, 0, 0, name.len() as u8 + 1];
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(&[0, kind]);
        for v in [n, 0x000D_DCE7, n, payload.len() as u32] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.extend_from_slice(packed);
        out
    }

    #[test]
    fn reads_records() {
        let long = format!("PS_{}_Wrapped", "x".repeat(40));
        let mut file = MAGIC.to_vec();
        file.extend_from_slice(&[1, 2, 3, 4]);
        file.extend(record_bytes(
            "VS_Blit_0011aabb_Wrapped",
            0x32,
            b"first container",
        ));
        // A marker whose sizes disagree is not a record.
        file.extend_from_slice(&[9, 0, 0, 0, 0xE7, 0xDC, 0x0D, 0, 8, 0, 0, 0]);
        file.extend(record_bytes(&long, 0x41, &[7; 300]));

        assert!(is_sgd5(&file));
        let found: Vec<_> = records(&file).collect();
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].name, Some("VS_Blit_0011aabb_Wrapped"));
        assert_eq!(found[0].kind, 0x32);
        assert_eq!(found[0].unpack().as_deref(), Some(&b"first container"[..]));
        assert_eq!(found[1].name, Some(long.as_str()));
        assert_eq!(found[1].unpack(), Some(vec![7; 300]));

        // The same file inside an archive.
        let mut archive = vec![0x5A; 0x1000];
        archive.extend_from_slice(&file);
        assert!(is_sgd5(&archive));
        assert_eq!(records(&archive).count(), 2);
        assert!(!is_sgd5(&archive[..0x1000]));
    }

    #[test]
    fn corrupt_stream_does_not_unpack() {
        let mut file = record_bytes("CS_x", 0x3c, &[1; 64]);
        file[27] = 0xFF; // first packed byte: reserved block type
        let r = records(&file).next().unwrap();
        assert_eq!(r.unpack(), None);
    }

    #[test]
    fn handles_empty_and_rejects_mismatched_or_truncated_streams() {
        let empty = record_bytes("CS_empty", 0x3c, &[]);
        assert_eq!(records(&empty).next().unwrap().unpack(), Some(Vec::new()));

        let payload: Vec<u8> = (0..16_384u32)
            .map(|i| (i.wrapping_mul(31) ^ (i >> 5)) as u8)
            .collect();
        let good = record_bytes("PS_big", 0x32, &payload);
        let r = records(&good).next().unwrap();
        assert_eq!(r.unpack(), Some(payload.clone()));

        // Oversized stream (declared unpacked_size smaller than actual output).
        let mut smaller = good.clone();
        smaller[r.offset + 8..r.offset + 12]
            .copy_from_slice(&((payload.len() - 1) as u32).to_le_bytes());
        assert_eq!(records(&smaller).next().unwrap().unpack(), None);

        // Under-filled stream (declared unpacked_size larger than actual output).
        let mut larger = good.clone();
        larger[r.offset + 8..r.offset + 12]
            .copy_from_slice(&((payload.len() + 1) as u32).to_le_bytes());
        assert_eq!(records(&larger).next().unwrap().unpack(), None);

        // Truncated raw deflate stream.
        let mut truncated = good[..good.len() - 1].to_vec();
        let new_n = (truncated.len() - r.offset) as u32;
        truncated[r.offset - 4..r.offset].copy_from_slice(&new_n.to_le_bytes());
        truncated[r.offset + 4..r.offset + 8].copy_from_slice(&new_n.to_le_bytes());
        assert_eq!(records(&truncated).next().unwrap().unpack(), None);
    }
}
