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
        unpack(self.packed, self.unpacked_size)
    }

    /// The container's shader hash ([`shader_hash`]), unpacking no more of
    /// the stream than it takes to reach it.
    pub fn shader_hash(&self) -> Option<[u8; 16]> {
        // The parts before `HASH` (features, signatures, PSV0) are small.
        const PREFIX: usize = 32 << 10;
        let prefix = match miniz_oxide::inflate::decompress_to_vec_with_limit(self.packed, PREFIX) {
            Ok(all) => return shader_hash(&all),
            Err(e) if matches!(e.status, miniz_oxide::inflate::TINFLStatus::HasMoreOutput) => e.output,
            Err(_) => return None,
        };
        shader_hash(&prefix).or_else(|| shader_hash(&self.unpack()?))
    }

    /// Where the record lies in its file, to read it again later; `window`
    /// is the bytes the record was found in, read from `window_offset`.
    pub fn place(&self, window: &[u8], window_offset: u64) -> Place {
        Place {
            packed_offset: window_offset + (self.packed.as_ptr() as usize - window.as_ptr() as usize) as u64,
            packed_size: self.packed.len() as u32,
            unpacked_size: self.unpacked_size as u32,
        }
    }
}

/// A record's packed stream in its file: enough to read and unpack it
/// again without scanning the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Place {
    pub packed_offset: u64,
    pub packed_size: u32,
    pub unpacked_size: u32,
}

impl Place {
    /// The container from the record's packed bytes.
    pub fn unpack(&self, packed: &[u8]) -> Option<Vec<u8>> {
        unpack(packed, self.unpacked_size as usize)
    }
}

fn unpack(packed: &[u8], size: usize) -> Option<Vec<u8>> {
    if size > MAX_UNPACKED {
        return None;
    }
    miniz_oxide::inflate::decompress_to_vec_with_limit(packed, size)
        .ok()
        .filter(|out| out.len() == size)
}

/// The digest of a shader container's `HASH` part (after its u32 of
/// flags): the shader's identity, which the console's compiled programs
/// also carry (see [`crate::pso_cache::code_shader_hash`]). `container` may be
/// cut short: `None` if the part is not within it.
pub fn shader_hash(container: &[u8]) -> Option<[u8; 16]> {
    let u32_at = |o: usize| {
        container
            .get(o..o + 4)
            .map(|b| u32::from_le_bytes(b.try_into().expect("4 bytes")) as usize)
    };
    if container.get(..4) != Some(b"DXBC") {
        return None;
    }
    let parts = u32_at(0x1c)?;
    if parts > 64 {
        return None;
    }
    (0..parts).find_map(|i| {
        let at = u32_at(0x20 + 4 * i)?;
        if container.get(at..at + 4) != Some(b"HASH") || u32_at(at + 4)? < 20 {
            return None;
        }
        container.get(at + 12..at + 28)?.try_into().ok()
    })
}

/// Records of a file read window by window, for files too big to hold
/// (the shader archive is 400 MB): `each` gets every record, with the
/// bytes it was found in and their offset in the file ([`Record::place`]).
pub fn scan<R: std::io::Read + std::io::Seek>(
    file: &mut R,
    each: impl FnMut(&[u8], u64, &Record<'_>),
) -> std::io::Result<()> {
    // Records are much smaller than the overlap; a bigger one is read on
    // its own.
    scan_windows(file, 32 << 20, 4 << 20, each)
}

fn scan_windows<R: std::io::Read + std::io::Seek>(
    file: &mut R,
    window: usize,
    overlap: usize,
    mut each: impl FnMut(&[u8], u64, &Record<'_>),
) -> std::io::Result<()> {
    use std::io::SeekFrom;
    let len = file.seek(SeekFrom::End(0))? as usize;
    let mut buffer = Vec::new();
    let mut start = 0usize;
    while start < len {
        // From a little before `start`: a record's length and kind precede
        // its marker.
        let from = start.saturating_sub(16);
        let to = (start + window + overlap).min(len);
        buffer.resize(to - from, 0);
        file.seek(SeekFrom::Start(from as u64))?;
        file.read_exact(&mut buffer)?;
        let end = start + window;
        let mut long = Vec::new();
        for at in memchr::memmem::find_iter(&buffer, &MARKER) {
            let marker = from + at;
            if marker < start || marker >= end {
                continue;
            }
            match record(&buffer, at) {
                Some(r) => each(&buffer, from as u64, &r),
                // Not all in this window: read it alone.
                None if to < len && marker >= 6 => long.push(marker),
                None => {}
            }
        }
        for marker in long {
            let mut head = [0u8; 12];
            file.seek(SeekFrom::Start((marker - 4) as u64))?;
            file.read_exact(&mut head)?;
            let word = |i: usize| u32::from_le_bytes(head[4 * i..4 * i + 4].try_into().expect("4 bytes")) as usize;
            let n = word(0);
            if n != word(2) || n < 12 || n > MAX_UNPACKED || marker + n > len {
                continue;
            }
            let from = marker - 6;
            let mut one = vec![0u8; n + 6];
            file.seek(SeekFrom::Start(from as u64))?;
            file.read_exact(&mut one)?;
            if let Some(r) = record(&one, 6) {
                each(&one, from as u64, &r);
            }
        }
        start = end;
    }
    Ok(())
}

/// Every container's shader hash and where its record lies, the first
/// record of each hash (the archive holds duplicates with the same code).
pub fn index<R: std::io::Read + std::io::Seek>(file: &mut R) -> std::io::Result<Vec<([u8; 16], Place)>> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    scan(file, |window, offset, r| {
        if let Some(hash) = r.shader_hash()
            && seen.insert(hash)
        {
            out.push((hash, r.place(window, offset)));
        }
    })?;
    Ok(out)
}

const INDEX_MAGIC: &[u8; 8] = b"SGD5IDX1";
const INDEX_ENTRY: usize = 32;

/// An [`index`] as bytes, for a file of `len` bytes last written at `stamp`
/// (to tell whether it is still the same file).
pub fn write_index(entries: &[([u8; 16], Place)], len: u64, stamp: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(28 + INDEX_ENTRY * entries.len());
    out.extend_from_slice(INDEX_MAGIC);
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(&stamp.to_le_bytes());
    out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    for (hash, p) in entries {
        out.extend_from_slice(hash);
        out.extend_from_slice(&p.packed_offset.to_le_bytes());
        out.extend_from_slice(&p.packed_size.to_le_bytes());
        out.extend_from_slice(&p.unpacked_size.to_le_bytes());
    }
    out
}

/// The entries [`write_index`] wrote, if they are for a file of `len` bytes
/// written at `stamp`.
pub fn read_index(bytes: &[u8], len: u64, stamp: u64) -> Option<Vec<([u8; 16], Place)>> {
    let u64_at = |o: usize| bytes.get(o..o + 8).map(|b| u64::from_le_bytes(b.try_into().expect("8 bytes")));
    let u32_at = |o: usize| bytes.get(o..o + 4).map(|b| u32::from_le_bytes(b.try_into().expect("4 bytes")));
    if bytes.get(..8) != Some(INDEX_MAGIC) || u64_at(8)? != len || u64_at(16)? != stamp {
        return None;
    }
    let count = u32_at(24)? as usize;
    if bytes.len() != 28 + INDEX_ENTRY * count {
        return None;
    }
    (0..count)
        .map(|i| {
            let at = 28 + INDEX_ENTRY * i;
            Some((
                bytes[at..at + 16].try_into().ok()?,
                Place {
                    packed_offset: u64_at(at + 16)?,
                    packed_size: u32_at(at + 24)?,
                    unpacked_size: u32_at(at + 28)?,
                },
            ))
        })
        .collect()
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
        let packed = miniz_oxide::deflate::compress_to_vec(payload, 6);
        let n = (packed.len() + 12) as u32;
        let mut out = vec![0xF4, 0x01, 0, 0, name.len() as u8 + 1];
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(&[0, kind]);
        for v in [n, 0x000D_DCE7, n, payload.len() as u32] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.extend_from_slice(&packed);
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

    /// A container of `parts` (four-character code, data).
    fn container(parts: &[(&[u8; 4], Vec<u8>)]) -> Vec<u8> {
        let head = 0x20 + 4 * parts.len();
        let mut out = b"DXBC".to_vec();
        out.extend_from_slice(&[0; 16]);
        out.extend_from_slice(&1u32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&(parts.len() as u32).to_le_bytes());
        let mut at = head;
        let mut body = Vec::new();
        for (code, data) in parts {
            out.extend_from_slice(&(at as u32).to_le_bytes());
            body.extend_from_slice(*code);
            body.extend_from_slice(&(data.len() as u32).to_le_bytes());
            body.extend_from_slice(data);
            at += 8 + data.len();
        }
        out.extend(body);
        let size = out.len() as u32;
        out[0x18..0x1c].copy_from_slice(&size.to_le_bytes());
        out
    }

    fn hash_part(digest: [u8; 16]) -> Vec<u8> {
        let mut p = 0u32.to_le_bytes().to_vec();
        p.extend_from_slice(&digest);
        p
    }

    #[test]
    fn shader_hash_is_the_hash_part_digest() {
        let c = container(&[(b"ISG1", vec![1; 40]), (b"HASH", hash_part([0x5a; 16])), (b"DXIL", vec![2; 4000])]);
        assert_eq!(shader_hash(&c), Some([0x5a; 16]));
        // Cut after the part: still found; cut inside it: not.
        let end = 0x20 + 12 + 48 + 8 + 20;
        assert_eq!(shader_hash(&c[..end]), Some([0x5a; 16]));
        assert_eq!(shader_hash(&c[..end - 1]), None);
        assert_eq!(shader_hash(&container(&[(b"DXIL", vec![2; 8])])), None);
        assert_eq!(shader_hash(b"DXBX"), None);
    }

    #[test]
    fn index_scans_windows_and_reads_records_again() {
        use std::io::{Cursor, Read, Seek, SeekFrom};
        // Records of incompressible data, so some cross window edges and
        // one is longer than the overlap.
        let noise = |seed: u32, n: usize| -> Vec<u8> {
            let mut x = seed.wrapping_mul(2_654_435_761) | 1;
            (0..n)
                .map(|_| {
                    x ^= x << 13;
                    x ^= x >> 17;
                    x ^= x << 5;
                    x as u8
                })
                .collect()
        };
        let mut file = MAGIC.to_vec();
        let mut want = Vec::new();
        for i in 0..12u8 {
            let digest = [i; 16];
            let c = container(&[(b"HASH", hash_part(digest)), (b"DXIL", noise(i.into(), 300 + 700 * usize::from(i)))]);
            file.extend(record_bytes(&format!("PS_x{i}_Wrapped"), 0x32, &c));
            want.push((digest, c));
        }
        // A duplicate of the first is indexed once.
        file.extend(record_bytes("PS_dup_Wrapped", 0x32, &want[0].1));
        let mut found = Vec::new();
        scan_windows(&mut Cursor::new(&file), 2000, 3000, |w, at, r| {
            found.push((r.shader_hash().unwrap(), r.place(w, at)));
        })
        .unwrap();
        assert_eq!(found.len(), 13);
        let entries = index(&mut Cursor::new(&file)).unwrap();
        assert_eq!(entries.len(), 12);
        for ((digest, place), (d, c)) in entries.iter().zip(&want) {
            assert_eq!(digest, d);
            let mut packed = vec![0; place.packed_size as usize];
            let mut f = Cursor::new(&file);
            f.seek(SeekFrom::Start(place.packed_offset)).unwrap();
            f.read_exact(&mut packed).unwrap();
            assert_eq!(place.unpack(&packed).as_ref(), Some(c));
        }
        // The small windows found the same places as one big one.
        let mut big = Vec::new();
        scan(&mut Cursor::new(&file), |w, at, r| big.push((r.shader_hash().unwrap(), r.place(w, at)))).unwrap();
        assert_eq!(big, found);

        let bytes = write_index(&entries, file.len() as u64, 77);
        assert_eq!(read_index(&bytes, file.len() as u64, 77), Some(entries));
        assert_eq!(read_index(&bytes, file.len() as u64, 78), None);
        assert_eq!(read_index(&bytes[..bytes.len() - 1], file.len() as u64, 77), None);
    }

    #[test]
    fn corrupt_stream_does_not_unpack() {
        let mut file = record_bytes("CS_x", 0x3c, &[1; 64]);
        file[27] = 0xFF; // first packed byte: reserved block type
        let r = records(&file).next().unwrap();
        assert_eq!(r.unpack(), None);
    }
}
