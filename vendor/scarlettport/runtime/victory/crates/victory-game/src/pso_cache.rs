//! The title's precompiled pipelines: `platform:/shaders/v2.d3d12PipelineCache`
//! (the title appends the extension at title+0x554cc50 and loads the file
//! at title+0x248e070). It is the last file of the shader archive, stored as
//! a chain of zlib streams of 64 KiB each. Its records are what the title
//! hands the console device's `DeserializeGraphicsPipelineStateX` and
//! `DeserializeComputePipelineStateX` (device slots 0x1c8 and 0x218):
//!
//! ```text
//! +0x00 u32 0x09005350, char[32] build ("0x65F41EDD"), char[32] driver,
//!       u64, then u32 counts: graphics records (0x70 B), compute records
//!       (0x18 B), blocks, codes, block bytes, code bytes; 0x80 bytes in all
//! graphics u64 key, u32 x2 state blocks, u64 x5 hashes, 7 stage slots of
//!          (u32 code, u32 metadata block), 0xffffffff where unused
//! compute  u64 key, u32 x2 state blocks, u32 code, u32 metadata block
//! blocks   (u32 offset, u32 size) into the block bytes; codes likewise
//!          into the code bytes (0x100-aligned; the title copies them into
//!          GPU memory)
//! ```
//!
//! Stage slots, by the programs they hold: 0 pixel, 1 vertex, 2 mesh; slot 6
//! seen twice, unknown. The code is native GFX10 compiled for the pipeline
//! (not the archive's programs), but it names its source: the first six
//! dwords are two VOP3 instructions whose operands spell the container's
//! `HASH` digest ([`code_shader_hash`]); the metadata block ends with the
//! same digest (entry 0x43, [`meta_shader_hash`]).

/// The file's first word.
pub const MAGIC: u32 = 0x0900_5350;
const HEADER: usize = 0x80;
const GRAPHICS_RECORD: usize = 0x70;
const COMPUTE_RECORD: usize = 0x18;
const NONE: u32 = u32::MAX;
/// Stage slots of a graphics record.
pub const GRAPHICS_SLOTS: usize = 7;

/// The stage a graphics stage slot holds, as the pipeline code names
/// stages; `None` for slots not seen holding a known stage.
pub fn slot_stage(slot: usize) -> Option<&'static str> {
    match slot {
        0 => Some("pixel"),
        1 => Some("vertex"),
        2 => Some("mesh"),
        _ => None,
    }
}

/// A parsed cache file (borrowing its bytes).
#[derive(Debug, Clone, Copy)]
pub struct Cache<'a> {
    pub build: &'a str,
    pub driver: &'a str,
    graphics: &'a [u8],
    compute: &'a [u8],
    blocks: &'a [u8],
    codes: &'a [u8],
    block_bytes: &'a [u8],
    code_bytes: &'a [u8],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Graphics {
    pub key: u64,
    pub state: [u32; 2],
    pub hashes: [u64; 5],
    /// Per slot: (code, metadata block), indices into the tables.
    pub stages: [Option<(u32, u32)>; GRAPHICS_SLOTS],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Compute {
    pub key: u64,
    pub state: [u32; 2],
    pub code: u32,
    pub meta: u32,
}

fn u32_at(b: &[u8], o: usize) -> Option<u32> {
    b.get(o..o + 4).map(|b| u32::from_le_bytes(b.try_into().expect("4 bytes")))
}

fn u64_at(b: &[u8], o: usize) -> Option<u64> {
    b.get(o..o + 8).map(|b| u64::from_le_bytes(b.try_into().expect("8 bytes")))
}

fn text(b: &[u8]) -> &str {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    std::str::from_utf8(&b[..end]).unwrap_or("")
}

impl<'a> Cache<'a> {
    /// The file, if its header and tables fit it exactly.
    pub fn parse(file: &'a [u8]) -> Result<Cache<'a>, &'static str> {
        if u32_at(file, 0) != Some(MAGIC) || file.len() < HEADER {
            return Err("not a pipeline cache");
        }
        let count = |i: usize| u32_at(file, 0x50 + 4 * i).expect("in the header") as usize;
        let sizes = [
            count(0) * GRAPHICS_RECORD,
            count(1) * COMPUTE_RECORD,
            count(2) * 8,
            count(3) * 8,
            count(4),
            count(5),
        ];
        if HEADER + sizes.iter().sum::<usize>() != file.len() {
            return Err("pipeline cache tables do not add up to its size");
        }
        let mut at = HEADER;
        let mut take = |n: usize| {
            let part = &file[at..at + n];
            at += n;
            part
        };
        Ok(Cache {
            build: text(&file[4..0x24]),
            driver: text(&file[0x24..0x44]),
            graphics: take(sizes[0]),
            compute: take(sizes[1]),
            blocks: take(sizes[2]),
            codes: take(sizes[3]),
            block_bytes: take(sizes[4]),
            code_bytes: take(sizes[5]),
        })
    }

    /// The total size the header of a file says it has (to know when a
    /// chain of streams is complete).
    pub fn size_of(header: &[u8]) -> Option<usize> {
        if u32_at(header, 0)? != MAGIC {
            return None;
        }
        let count = |i: usize| u32_at(header, 0x50 + 4 * i).map(|c| c as usize);
        Some(
            HEADER
                + count(0)? * GRAPHICS_RECORD
                + count(1)? * COMPUTE_RECORD
                + count(2)? * 8
                + count(3)? * 8
                + count(4)?
                + count(5)?,
        )
    }

    pub fn graphics_count(&self) -> usize {
        self.graphics.len() / GRAPHICS_RECORD
    }

    pub fn compute_count(&self) -> usize {
        self.compute.len() / COMPUTE_RECORD
    }

    pub fn graphics(&self, i: usize) -> Option<Graphics> {
        let r = self.graphics.get(i * GRAPHICS_RECORD..(i + 1) * GRAPHICS_RECORD)?;
        let mut stages = [None; GRAPHICS_SLOTS];
        for (s, slot) in stages.iter_mut().enumerate() {
            let (code, meta) = (u32_at(r, 0x38 + 8 * s)?, u32_at(r, 0x3c + 8 * s)?);
            *slot = (code != NONE).then_some((code, meta));
        }
        Some(Graphics {
            key: u64_at(r, 0)?,
            state: [u32_at(r, 8)?, u32_at(r, 12)?],
            hashes: [0, 1, 2, 3, 4].map(|k| u64_at(r, 0x10 + 8 * k).expect("in the record")),
            stages,
        })
    }

    pub fn compute(&self, i: usize) -> Option<Compute> {
        let r = self.compute.get(i * COMPUTE_RECORD..(i + 1) * COMPUTE_RECORD)?;
        Some(Compute {
            key: u64_at(r, 0)?,
            state: [u32_at(r, 8)?, u32_at(r, 12)?],
            code: u32_at(r, 16)?,
            meta: u32_at(r, 20)?,
        })
    }

    /// A state or metadata block.
    pub fn block(&self, i: u32) -> Option<&'a [u8]> {
        slice(self.blocks, self.block_bytes, i)
    }

    /// A stage's native code.
    pub fn code(&self, i: u32) -> Option<&'a [u8]> {
        slice(self.codes, self.code_bytes, i)
    }
}

fn slice<'a>(table: &[u8], bytes: &'a [u8], i: u32) -> Option<&'a [u8]> {
    let at = 8 * i as usize;
    let (offset, size) = (u32_at(table, at)? as usize, u32_at(table, at + 4)? as usize);
    bytes.get(offset..offset.checked_add(size)?)
}

/// The shader digest a native program's first two instructions spell
/// (`v_nop`-like VOP3 instructions with a literal: low half of the first
/// dword, then the low bytes of `src0` and `src1`, then the literal).
pub fn code_shader_hash(code: &[u8]) -> Option<[u8; 16]> {
    let w: Vec<u32> = (0..6).map(|i| u32_at(code, 4 * i)).collect::<Option<_>>()?;
    if w[0] >> 16 != 0xd580 || w[3] >> 16 != 0xd580 {
        return None;
    }
    let half = |a: u32, b: u32| (a & 0xffff) | (b & 0xff) << 16 | (b >> 9 & 0xff) << 24;
    let mut out = [0u8; 16];
    for (i, d) in [half(w[0], w[1]), w[2], half(w[3], w[4]), w[5]].into_iter().enumerate() {
        out[4 * i..4 * i + 4].copy_from_slice(&d.to_le_bytes());
    }
    Some(out)
}

/// The shader digest a metadata block ends with (entry 0x43). Graphics
/// blocks have a 16-byte head (u32 2, u32 size used, two words), compute
/// blocks a 24-byte head (u32 0x403, ...); entries 0x41 (descriptor ranges
/// read) and 0x42 (user-data layout) are 0x20 bytes, 0x43 is 0x14.
pub fn meta_shader_hash(meta: &[u8]) -> Option<[u8; 16]> {
    let mut at = match u32_at(meta, 0)? {
        2 => 16,
        0x403 => 24,
        _ => return None,
    };
    loop {
        match u32_at(meta, at)? {
            0x41 | 0x42 => at += 0x20,
            0x43 => return meta.get(at + 4..at + 20)?.try_into().ok(),
            _ => return None,
        }
    }
}

/// The digest a stage names: its metadata's, else its code's; `None` if
/// neither has one or they disagree.
pub fn stage_shader_hash(code: &[u8], meta: &[u8]) -> Option<[u8; 16]> {
    match (meta_shader_hash(meta), code_shader_hash(code)) {
        (Some(m), Some(c)) if m != c => None,
        (Some(h), _) | (None, Some(h)) => Some(h),
        (None, None) => None,
    }
}

/// The cache file inside the shader archive: the chain of zlib streams
/// whose first unpacks to a cache header, each next stream starting at most
/// 64 bytes after the end of the one before.
pub fn find_in_archive(archive: &[u8]) -> Option<Vec<u8>> {
    let mut from = 0;
    while let Some(at) = memchr::memmem::find(&archive[from..], &[0x78]).map(|p| from + p) {
        from = at + 1;
        let Some((first, _)) = inflate_one(&archive[at..], 0x80) else {
            continue;
        };
        if u32_at(&first, 0) != Some(MAGIC) {
            continue;
        }
        return chain(archive, at);
    }
    None
}

/// The streams from `at` on, until the header's size is reached.
fn chain(archive: &[u8], mut at: usize) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut want = usize::MAX;
    while out.len() < want {
        let (part, used) = inflate_one(archive.get(at..)?, usize::MAX)?;
        out.extend_from_slice(&part);
        if want == usize::MAX && out.len() >= HEADER {
            want = Cache::size_of(&out)?;
        }
        if out.len() >= want {
            break;
        }
        let next = at + used;
        at = (next..(next + 64).min(archive.len().saturating_sub(1)))
            .find(|&i| archive[i] == 0x78 && (u16::from(archive[i]) << 8 | u16::from(archive[i + 1])) % 31 == 0)?;
    }
    (out.len() == want).then_some(out)
}

/// One zlib stream at the start of `input`: its bytes (at most `limit`) and
/// how many input bytes it took (when unpacked whole).
fn inflate_one(input: &[u8], limit: usize) -> Option<(Vec<u8>, usize)> {
    use miniz_oxide::inflate::stream::{InflateState, inflate};
    use miniz_oxide::{DataFormat, MZFlush, MZStatus};
    let mut state = InflateState::new_boxed(DataFormat::Zlib);
    let mut out = Vec::new();
    let mut buf = vec![0u8; 1 << 16];
    let mut used = 0;
    loop {
        let r = inflate(&mut state, &input[used..], &mut buf, MZFlush::None);
        used += r.bytes_consumed;
        out.extend_from_slice(&buf[..r.bytes_written]);
        if out.len() >= limit {
            out.truncate(limit);
            return Some((out, used));
        }
        match r.status {
            Ok(MZStatus::StreamEnd) => return Some((out, used)),
            Ok(_) if r.bytes_consumed > 0 || r.bytes_written > 0 => {}
            _ => return None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A code block whose prologue spells `hash`.
    pub(crate) fn code_with(hash: [u8; 16], body: &[u32]) -> Vec<u8> {
        let d = |i: usize| u32::from_le_bytes(hash[4 * i..4 * i + 4].try_into().unwrap());
        let ops = |x: u32| 0xff << 18 | (0x100 | (x >> 24 & 0xff)) << 9 | 0x100 | (x >> 16 & 0xff);
        let mut w = vec![0xd580_0000 | (d(0) & 0xffff), ops(d(0)), d(1), 0xd580_0000 | (d(2) & 0xffff), ops(d(2)), d(3)];
        w.extend_from_slice(body);
        w.extend([0xbf9f_0000; 4]);
        w.iter().flat_map(|x| x.to_le_bytes()).collect()
    }

    fn meta_with(head: &[u32], hash: [u8; 16]) -> Vec<u8> {
        let mut m: Vec<u8> = head.iter().flat_map(|x| x.to_le_bytes()).collect();
        for kind in [0x41u32, 0x42] {
            m.extend_from_slice(&kind.to_le_bytes());
            m.extend_from_slice(&[7; 0x1c]);
        }
        m.extend_from_slice(&0x43u32.to_le_bytes());
        m.extend_from_slice(&hash);
        m.extend_from_slice(&[0; 12]);
        m
    }

    /// A cache of one graphics pipeline (pixel + mesh) and one compute one.
    pub(crate) fn cache_bytes(hashes: [[u8; 16]; 3]) -> Vec<u8> {
        let blocks: Vec<Vec<u8>> = vec![
            vec![1; 0x184],
            vec![2; 0x50],
            meta_with(&[2, 0, 0x76, 0], hashes[0]),
            meta_with(&[2, 0, 0x76, 0], hashes[1]),
            vec![3; 0x1c],
            vec![4; 0x14],
            meta_with(&[0x403, 0, 0x60, 0, 0, 0], hashes[2]),
        ];
        let codes: Vec<Vec<u8>> = hashes.iter().map(|h| code_with(*h, &[0xbfa0_0003, 0xbf81_0000])).collect();
        let table = |parts: &[Vec<u8>], align: usize| {
            let (mut t, mut bytes) = (Vec::new(), Vec::new());
            for p in parts {
                bytes.resize(bytes.len().next_multiple_of(align), 0);
                t.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
                t.extend_from_slice(&(p.len() as u32).to_le_bytes());
                bytes.extend_from_slice(p);
            }
            (t, bytes)
        };
        let (bt, bb) = table(&blocks, 4);
        let (ct, cb) = table(&codes, 0x100);
        let mut g = Vec::new();
        g.extend_from_slice(&0x1122u64.to_le_bytes());
        g.extend_from_slice(&0u32.to_le_bytes());
        g.extend_from_slice(&1u32.to_le_bytes());
        for k in 0..5u64 {
            g.extend_from_slice(&(0x100 + k).to_le_bytes());
        }
        for slot in 0..GRAPHICS_SLOTS {
            let (c, m) = match slot {
                0 => (0, 2),
                2 => (1, 3),
                _ => (NONE, NONE),
            };
            g.extend_from_slice(&c.to_le_bytes());
            g.extend_from_slice(&m.to_le_bytes());
        }
        let mut c = Vec::new();
        for v in [0x3344u64] {
            c.extend_from_slice(&v.to_le_bytes());
        }
        for v in [4u32, 5, 2, 6] {
            c.extend_from_slice(&v.to_le_bytes());
        }
        let mut f = vec![0u8; HEADER];
        f[..4].copy_from_slice(&MAGIC.to_le_bytes());
        f[4..14].copy_from_slice(b"0x65F41EDD");
        f[0x24..0x2f].copy_from_slice(b"930.048.020");
        for (i, n) in [1, 1, blocks.len(), codes.len(), bb.len(), cb.len()].into_iter().enumerate() {
            f[0x50 + 4 * i..0x54 + 4 * i].copy_from_slice(&(n as u32).to_le_bytes());
        }
        for part in [g, c, bt, ct, bb, cb] {
            f.extend_from_slice(&part);
        }
        f
    }

    const H: [[u8; 16]; 3] = [[0xa1; 16], [0xb2; 16], [0xc3; 16]];

    #[test]
    fn parses_records_blocks_and_codes() {
        let file = cache_bytes(H);
        let cache = Cache::parse(&file).unwrap();
        assert_eq!((cache.build, cache.driver), ("0x65F41EDD", "930.048.020"));
        assert_eq!((cache.graphics_count(), cache.compute_count()), (1, 1));
        assert_eq!(Cache::size_of(&file[..HEADER]), Some(file.len()));
        let g = cache.graphics(0).unwrap();
        assert_eq!(g.key, 0x1122);
        assert_eq!(g.hashes[4], 0x104);
        assert_eq!(cache.block(g.state[0]).unwrap().len(), 0x184);
        let used: Vec<usize> = (0..GRAPHICS_SLOTS).filter(|&s| g.stages[s].is_some()).collect();
        assert_eq!(used, [0, 2]);
        assert_eq!(used.iter().map(|&s| slot_stage(s).unwrap()).collect::<Vec<_>>(), ["pixel", "mesh"]);
        for (slot, want) in [(0, H[0]), (2, H[1])] {
            let (code, meta) = g.stages[slot].unwrap();
            let (code, meta) = (cache.code(code).unwrap(), cache.block(meta).unwrap());
            assert_eq!(stage_shader_hash(code, meta), Some(want));
        }
        let c = cache.compute(0).unwrap();
        assert_eq!((c.key, c.state), (0x3344, [4, 5]));
        assert_eq!(cache.block(c.state[0]).unwrap().len(), 0x1c);
        assert_eq!(stage_shader_hash(cache.code(c.code).unwrap(), cache.block(c.meta).unwrap()), Some(H[2]));
        assert!(cache.graphics(1).is_none() && cache.code(9).is_none());
        // A size that disagrees with the tables is refused.
        assert!(Cache::parse(&file[..file.len() - 1]).is_err());
    }

    #[test]
    fn the_code_prologue_spells_the_digest_of_the_title_s_first_pipeline() {
        // The first graphics stage of the title's file: pixel code and the
        // end of its metadata block.
        let code: Vec<u8> = [0xd5808d2eu32, 0x03ff6972, 0x8496ce98, 0xd5806453, 0x03ff6b70, 0x1de7a8f1, 0xd5800000]
            .iter()
            .flat_map(|w| w.to_le_bytes())
            .collect();
        let want: [u8; 16] = [
            0x2e, 0x8d, 0x72, 0xb4, 0x98, 0xce, 0x96, 0x84, 0x53, 0x64, 0x70, 0xb5, 0xf1, 0xa8, 0xe7, 0x1d,
        ];
        assert_eq!(code_shader_hash(&code), Some(want));
        assert_eq!(code_shader_hash(&code[4..]), None);
        // Metadata and code that disagree name nothing.
        let meta = meta_with(&[2, 0, 0x76, 0], [9; 16]);
        assert_eq!(meta_shader_hash(&meta), Some([9; 16]));
        assert_eq!(stage_shader_hash(&code, &meta), None);
        assert_eq!(stage_shader_hash(&code, &[]), Some(want));
        assert_eq!(meta_shader_hash(&[5, 0, 0, 0]), None);
    }

    #[test]
    fn finds_the_chain_of_streams_in_an_archive() {
        let file = cache_bytes(H);
        let mut archive = vec![0x78u8, 0x9c, 0x00, 0x01, 0x55];
        archive.extend(miniz_oxide::deflate::compress_to_vec_zlib(b"another file", 6));
        // The cache in 100-byte pieces, with a few bytes between streams.
        for (i, piece) in file.chunks(100).enumerate() {
            archive.extend(miniz_oxide::deflate::compress_to_vec_zlib(piece, 9));
            archive.extend(std::iter::repeat_n(0u8, i % 13));
        }
        assert_eq!(find_in_archive(&archive).as_deref(), Some(&file[..]));
        // A chain that stops early is no file.
        let cut = archive.len() - 40;
        assert_eq!(find_in_archive(&archive[..cut]), None);
    }
}
