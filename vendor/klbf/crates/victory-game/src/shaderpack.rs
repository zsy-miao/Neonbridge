//! The shader pack: the engine's pipeline cache, shipped beside the shader
//! databases. Each entry is one pipeline as the console driver serializes
//! it (a register packet, pipeline metadata and, per hardware stage, the
//! native program and its metadata), keyed by the engine's own hashes.
//! Layout (little-endian):
//!
//! ```text
//! header 0x80 B: magic, char[32] driver build, char[36] compiler version,
//!   +0x48 u64 fingerprint of the shader database, u32 graphics count,
//!   u32 compute count, u32 blob count, u32 program count,
//!   u32 blob bytes, u32 program bytes
//! graphics 0x70 B: u64 key, u32 packet blob, u32 metadata blob,
//!   u64 vertex (or mesh) shader hash, u64 pipeline hash,
//!   u64 depth-stencil, rasterizer and blend state hashes,
//!   7 x {u32 program, u32 program metadata blob}: PS VS GS ES HS LS fetch
//! compute 0x18 B: u64 shader hash, u32 packet blob, u32 metadata blob,
//!   u32 program, u32 program metadata blob
//! blob table, program table: u32 offset, u32 size each
//! blob data, then program data (programs at 256-byte offsets)
//! ```
//!
//! Absent indices are `u32::MAX`. The shader hashes are the ones the shader
//! database stores after each packed container; the state hashes are 64-bit
//! CityHash (v1.1) of the engine's compact state descriptions.
//!
//! Graphics metadata also carries part of the pipeline description:
//! `+0x10` eight render-target formats (DXGI, one byte each), `+0x1c` the
//! sample mask, `+0x24` the depth format, render-target count and topology
//! type (bytes), `+0x28` sample count and quality, `+0x38` the size of the
//! input layout at `+0x4c`: u32 element count, then per element u32 name
//! length (with NUL), name, u32 semantic index, format, input slot, byte
//! offset, slot class, step rate.

pub const MAGIC: &[u8; 4] = b"PS\0\x09";
const HEADER: usize = 0x80;
const GRAPHICS: usize = 0x70;
const COMPUTE: usize = 0x18;
const NONE: u32 = u32::MAX;
/// Hardware stages of a graphics entry, in its slot order.
pub const STAGES: [&str; 7] = ["PS", "VS", "GS", "ES", "HS", "LS", "fetch"];

fn u32_at(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}

fn u64_at(b: &[u8], o: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(o..o + 8)?.try_into().ok()?))
}

fn text(b: &[u8]) -> &str {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    std::str::from_utf8(&b[..end]).unwrap_or("?")
}

/// A parsed pack; every record and table entry was checked to lie inside
/// the file.
pub struct Pack<'a> {
    file: &'a [u8],
    graphics: &'a [u8],
    compute: &'a [u8],
    blobs: Table<'a>,
    programs: Table<'a>,
}

#[derive(Clone, Copy)]
struct Table<'a> {
    entries: &'a [u8],
    data: &'a [u8],
}

impl<'a> Table<'a> {
    fn get(&self, index: u32) -> Option<&'a [u8]> {
        if index == NONE {
            return None;
        }
        let at = index as usize * 8;
        let off = u32_at(self.entries, at)? as usize;
        let len = u32_at(self.entries, at + 4)? as usize;
        self.data.get(off..off.checked_add(len)?)
    }

    fn len(&self) -> u32 {
        (self.entries.len() / 8) as u32
    }
}

/// One stage of a pipeline: native code and its metadata.
#[derive(Debug, Clone, Copy)]
pub struct Stage<'a> {
    pub program: &'a [u8],
    pub metadata: Option<&'a [u8]>,
}

#[derive(Debug, Clone, Copy)]
pub struct Graphics<'a> {
    /// What the engine looks a pipeline up by: a one-way hash of the shader
    /// set, input layout, target formats and write masks.
    pub key: u64,
    /// Database hash of the vertex or mesh shader; the engine creates every
    /// entry with this hash once that shader's root signature exists.
    pub shader_hash: u64,
    /// Hash of the whole pipeline description.
    pub pipeline_hash: u64,
    pub depth_stencil_hash: u64,
    pub rasterizer_hash: u64,
    pub blend_hash: u64,
    pub packet: &'a [u8],
    pub metadata: &'a [u8],
    /// Indexed like [`STAGES`].
    pub stages: [Option<Stage<'a>>; 7],
}

#[derive(Debug, Clone, Copy)]
pub struct Compute<'a> {
    /// Database hash of the compute shader (the lookup key).
    pub shader_hash: u64,
    pub packet: &'a [u8],
    pub metadata: &'a [u8],
    pub stage: Stage<'a>,
}

/// One input-layout element of a graphics entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputElement<'a> {
    pub semantic: &'a str,
    pub index: u32,
    /// DXGI format.
    pub format: u32,
    pub slot: u32,
    pub offset: u32,
    /// 0 per vertex, 1 per instance.
    pub class: u32,
    pub step_rate: u32,
}

impl<'a> Graphics<'a> {
    /// Render-target formats (DXGI), as stored: unused targets are 0.
    pub fn target_formats(&self) -> Option<[u8; 8]> {
        self.metadata.get(0x10..0x18)?.try_into().ok()
    }

    /// Depth format (DXGI), render-target count and topology type.
    pub fn depth_targets_topology(&self) -> Option<(u8, u8, u8)> {
        let b = self.metadata.get(0x24..0x27)?;
        Some((b[0], b[1], b[2]))
    }

    /// Sample mask, sample count and quality.
    pub fn samples(&self) -> Option<(u32, u32, u32)> {
        Some((
            u32_at(self.metadata, 0x1c)?,
            u32_at(self.metadata, 0x28)?,
            u32_at(self.metadata, 0x2c)?,
        ))
    }

    /// The input layout; `None` if the metadata does not read as one.
    pub fn input_layout(&self) -> Option<Vec<InputElement<'a>>> {
        let m = self.metadata;
        let end = 0x4c + u32_at(m, 0x38)? as usize;
        let count = u32_at(m, 0x4c)?;
        let mut at = 0x50;
        let mut out = Vec::new();
        for _ in 0..count {
            let len = u32_at(m, at)? as usize;
            let name = m.get(at + 4..at + 4 + len)?;
            at += 4 + len;
            let f = |i: usize| u32_at(m, at + 4 * i);
            out.push(InputElement {
                semantic: text(name),
                index: f(0)?,
                format: f(1)?,
                slot: f(2)?,
                offset: f(3)?,
                class: f(4)?,
                step_rate: f(5)?,
            });
            at += 24;
        }
        (at == end && end <= m.len()).then_some(out)
    }
}

impl<'a> Pack<'a> {
    pub fn parse(file: &'a [u8]) -> Result<Self, &'static str> {
        if !file.starts_with(MAGIC) {
            return Err("not a shader pack");
        }
        let n = |o| u32_at(file, o).map(|v| v as usize).ok_or("truncated header");
        let (ng, nc, nb, np, blob_bytes, prog_bytes) =
            (n(0x50)?, n(0x54)?, n(0x58)?, n(0x5c)?, n(0x60)?, n(0x64)?);
        let mut at = HEADER;
        let mut take = |len: usize| {
            let part = file.get(at..at.checked_add(len)?)?;
            at += len;
            Some(part)
        };
        let graphics = take(ng * GRAPHICS).ok_or("truncated graphics records")?;
        let compute = take(nc * COMPUTE).ok_or("truncated compute records")?;
        let blob_entries = take(nb * 8).ok_or("truncated blob table")?;
        let prog_entries = take(np * 8).ok_or("truncated program table")?;
        let blob_data = take(blob_bytes).ok_or("truncated blob data")?;
        let prog_data = take(prog_bytes).ok_or("truncated program data")?;
        let pack = Pack {
            file,
            graphics,
            compute,
            blobs: Table {
                entries: blob_entries,
                data: blob_data,
            },
            programs: Table {
                entries: prog_entries,
                data: prog_data,
            },
        };
        // Check every reference once so the iterators cannot fail.
        for i in 0..pack.blobs.len() {
            pack.blobs.get(i).ok_or("blob outside its data")?;
        }
        for i in 0..pack.programs.len() {
            pack.programs.get(i).ok_or("program outside its data")?;
        }
        let blob = |i: u32| i < pack.blobs.len();
        let prog = |i: u32| i == NONE || i < pack.programs.len();
        let meta = |i: u32| i == NONE || i < pack.blobs.len();
        for r in graphics.as_chunks::<GRAPHICS>().0 {
            let w = |o: usize| u32_at(r, o).unwrap_or(NONE);
            let ok = blob(w(8))
                && blob(w(12))
                && (0..7).all(|s| prog(w(0x38 + 8 * s)) && meta(w(0x3c + 8 * s)));
            if !ok {
                return Err("graphics record refers outside the tables");
            }
        }
        for r in compute.as_chunks::<COMPUTE>().0 {
            let w = |o| u32_at(r, o).unwrap_or(NONE);
            if !(blob(w(8)) && blob(w(12)) && w(16) < pack.programs.len() && meta(w(20))) {
                return Err("compute record refers outside the tables");
            }
        }
        Ok(pack)
    }

    /// The console driver build the pack was made with.
    pub fn driver(&self) -> &'a str {
        text(&self.file[4..0x24])
    }

    /// The shader compiler version string.
    pub fn compiler(&self) -> &'a str {
        text(&self.file[0x24..0x48])
    }

    /// Fingerprint of the shader database the pack was made for.
    pub fn fingerprint(&self) -> u64 {
        u64_at(self.file, 0x48).unwrap_or(0)
    }

    pub fn graphics(&self) -> impl Iterator<Item = Graphics<'a>> + '_ {
        self.graphics.as_chunks::<GRAPHICS>().0.iter().map(|r| {
            let q = |o| u64_at(r, o).unwrap_or(0);
            let w = |o| u32_at(r, o).unwrap_or(NONE);
            let stages = std::array::from_fn(|s| {
                Some(Stage {
                    program: self.programs.get(w(0x38 + 8 * s))?,
                    metadata: self.blobs.get(w(0x3c + 8 * s)),
                })
            });
            Graphics {
                key: q(0),
                shader_hash: q(0x10),
                pipeline_hash: q(0x18),
                depth_stencil_hash: q(0x20),
                rasterizer_hash: q(0x28),
                blend_hash: q(0x30),
                packet: self.blobs.get(w(8)).unwrap_or_default(),
                metadata: self.blobs.get(w(12)).unwrap_or_default(),
                stages,
            }
        })
    }

    pub fn compute(&self) -> impl Iterator<Item = Compute<'a>> + '_ {
        self.compute.as_chunks::<COMPUTE>().0.iter().map(|r| {
            let w = |o| u32_at(r, o).unwrap_or(NONE);
            Compute {
                shader_hash: u64_at(r, 0).unwrap_or(0),
                packet: self.blobs.get(w(8)).unwrap_or_default(),
                metadata: self.blobs.get(w(12)).unwrap_or_default(),
                stage: Stage {
                    program: self.programs.get(w(16)).unwrap_or_default(),
                    metadata: self.blobs.get(w(20)),
                },
            }
        })
    }
}

/// The pack in `file`: the file itself, or the pack an archive holds as a
/// run of zlib streams (each starting at a multiple of 16 bytes and
/// inflating to at most 64 KiB), found by its magic.
pub fn find(file: &[u8]) -> Option<std::borrow::Cow<'_, [u8]>> {
    if file.starts_with(MAGIC) {
        return Some(std::borrow::Cow::Borrowed(file));
    }
    let mut chunk = vec![0u8; 0x20000];
    // One zlib stream at `at`: its output and the bytes it took.
    let mut inflate = |at: usize| {
        let source = file.get(at..)?;
        let mut state = zlib_rs::Inflate::new(true, 15);
        loop {
            let (was_in, was_out) = (state.total_in(), state.total_out());
            let status = state
                .decompress(&source[was_in as usize..], &mut chunk[was_out as usize..], zlib_rs::InflateFlush::NoFlush)
                .ok()?;
            match status {
                zlib_rs::Status::StreamEnd => {
                    break Some((chunk[..state.total_out() as usize].to_vec(), state.total_in() as usize));
                }
                zlib_rs::Status::Ok if (state.total_in(), state.total_out()) != (was_in, was_out) => {}
                _ => break None,
            }
        }
    };
    let zlib = |at: usize| {
        file[at] == 0x78 && u16::from_be_bytes([file[at], file[at + 1]]).is_multiple_of(31)
    };
    let start = (0..file.len().saturating_sub(1))
        .step_by(16)
        .filter(|&at| zlib(at))
        .find_map(|at| inflate(at).filter(|(out, _)| out.starts_with(MAGIC)).map(|o| (at, o)))?;
    let (mut at, (mut pack, used)) = start;
    at += used;
    let size = |p: &[u8]| -> Option<usize> {
        let n = |o: usize| u32_at(p, o).map(|v| v as usize);
        let tables = n(0x50)? * GRAPHICS + n(0x54)? * COMPUTE + (n(0x58)? + n(0x5c)?) * 8;
        Some(HEADER + tables + n(0x60)? + n(0x64)?)
    };
    loop {
        // The header gives the size once it is whole.
        if let Some(total) = size(&pack).filter(|&t| pack.len() >= t) {
            return (pack.len() == total).then_some(std::borrow::Cow::Owned(pack));
        }
        at = at.next_multiple_of(16);
        let (out, used) = inflate(at)?;
        pack.extend(out);
        at += used;
    }
}

/// The two 12-byte instructions a native program starts with, which carry
/// its shader's hash: equal for the pack's program and the database
/// container it was compiled from.
pub fn program_id(code: &[u8]) -> Option<[u8; 24]> {
    let id: [u8; 24] = code.get(..24)?.try_into().ok()?;
    // VOP3 opcode 0x180 (v_nop) in both.
    let nop = |o: usize| u32::from_le_bytes(id[o..o + 4].try_into().expect("4")) >> 16 == 0xD580;
    (nop(0) && nop(12)).then_some(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zlib(plain: &[u8]) -> Vec<u8> {
        let mut buf = vec![0u8; zlib_rs::compress_bound(plain.len())];
        let (packed, rc) = zlib_rs::compress_slice(&mut buf, plain, zlib_rs::DeflateConfig::new(6));
        assert_eq!(rc, zlib_rs::ReturnCode::Ok);
        packed.to_vec()
    }

    fn le(out: &mut Vec<u8>, words: &[u32]) {
        for w in words {
            out.extend_from_slice(&w.to_le_bytes());
        }
    }

    fn metadata() -> Vec<u8> {
        let mut m = vec![0; 0x4c];
        m[0x10..0x18].copy_from_slice(&[28, 10, 0, 0, 0, 0, 0, 0]);
        m[0x1c..0x20].copy_from_slice(&u32::MAX.to_le_bytes());
        m[0x24..0x27].copy_from_slice(&[40, 2, 3]);
        m[0x28] = 1;
        let mut layout = Vec::new();
        le(&mut layout, &[2]);
        for (name, words) in [("POSITION", [0, 6, 0, 0, 0, 0]), ("COLOR", [1, 28, 1, 12, 1, 1])] {
            le(&mut layout, &[name.len() as u32 + 1]);
            layout.extend_from_slice(name.as_bytes());
            layout.push(0);
            le(&mut layout, &words);
        }
        m[0x38..0x3c].copy_from_slice(&(layout.len() as u32).to_le_bytes());
        m.extend(layout);
        m.resize(m.len().next_multiple_of(4), 0);
        m
    }

    /// A pack with one graphics entry (PS, VS) and one compute entry.
    fn pack_bytes() -> Vec<u8> {
        let nop = |h: u32| [0xD580_0000 | (h & 0xFFFF), 0x03FF_0000, h];
        let mut code0 = Vec::new();
        le(&mut code0, &nop(0x1234));
        le(&mut code0, &nop(0x5678));
        le(&mut code0, &[0xBF81_0000]);
        let programs = [code0, vec![1; 8], vec![2; 12]];
        let blobs = [b"packet".to_vec(), metadata(), b"ps meta".to_vec(), vec![3; 28], vec![4; 20]];
        let mut out = MAGIC.to_vec();
        out.extend(b"0x00000001".iter().chain(&[0; 22]));
        out.extend(b"1.2.3".iter().chain(&[0; 31]));
        out.extend(0xABCD_u64.to_le_bytes());
        let blob_bytes: usize = blobs.iter().map(Vec::len).sum();
        le(&mut out, &[1, 1, blobs.len() as u32, programs.len() as u32, blob_bytes as u32, 0x300]);
        out.resize(HEADER, 0);
        // Graphics: key, packet 0, metadata 1, five hashes, PS and VS.
        out.extend(0x1111_u64.to_le_bytes());
        le(&mut out, &[0, 1]);
        for h in 1..=5_u64 {
            out.extend(h.to_le_bytes());
        }
        le(&mut out, &[0, 2, 1, NONE]);
        le(&mut out, &[NONE; 10]);
        // Compute: shader hash, packet 3, metadata 4, program 2, no metadata.
        out.extend(0x2222_u64.to_le_bytes());
        le(&mut out, &[3, 4, 2, NONE]);
        let mut at = 0;
        for b in &blobs {
            le(&mut out, &[at, b.len() as u32]);
            at += b.len() as u32;
        }
        for (i, p) in programs.iter().enumerate() {
            le(&mut out, &[i as u32 * 0x100, p.len() as u32]);
        }
        for b in &blobs {
            out.extend(b);
        }
        let base = out.len();
        for (i, p) in programs.iter().enumerate() {
            out.resize(base + i * 0x100, 0);
            out.extend(p);
        }
        out.resize(base + 0x300, 0);
        out
    }

    #[test]
    fn reads_entries() {
        let file = pack_bytes();
        let pack = Pack::parse(&file).unwrap();
        assert_eq!(pack.driver(), "0x00000001");
        assert_eq!(pack.compiler(), "1.2.3");
        assert_eq!(pack.fingerprint(), 0xABCD);

        let g: Vec<_> = pack.graphics().collect();
        assert_eq!(g.len(), 1);
        let g = g[0];
        assert_eq!(g.key, 0x1111);
        let hashes = [g.shader_hash, g.pipeline_hash, g.depth_stencil_hash, g.rasterizer_hash];
        assert_eq!((hashes, g.blend_hash), ([1, 2, 3, 4], 5));
        assert_eq!(g.packet, b"packet");
        let ps = g.stages[0].unwrap();
        assert_eq!(ps.metadata, Some(&b"ps meta"[..]));
        assert_eq!(program_id(ps.program).unwrap()[..4], [0x34, 0x12, 0x80, 0xD5]);
        let vs = g.stages[1].unwrap();
        assert_eq!((vs.program, vs.metadata), (&[1; 8][..], None));
        assert!(g.stages[2..].iter().all(Option::is_none));
        assert_eq!(program_id(vs.program), None);

        assert_eq!(g.target_formats(), Some([28, 10, 0, 0, 0, 0, 0, 0]));
        assert_eq!(g.depth_targets_topology(), Some((40, 2, 3)));
        assert_eq!(g.samples(), Some((u32::MAX, 1, 0)));
        let layout = g.input_layout().unwrap();
        assert_eq!(layout.len(), 2);
        assert_eq!((layout[0].semantic, layout[0].format), ("POSITION", 6));
        let color = &layout[1];
        assert_eq!((color.semantic, color.index, color.slot, color.offset), ("COLOR", 1, 1, 12));
        assert_eq!((color.class, color.step_rate), (1, 1));

        let c: Vec<_> = pack.compute().collect();
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].shader_hash, 0x2222);
        assert_eq!((c[0].packet.len(), c[0].metadata.len()), (28, 20));
        assert_eq!((c[0].stage.program, c[0].stage.metadata), (&[2; 12][..], None));
    }

    #[test]
    fn finds_a_pack_inside_an_archive() {
        let pack = pack_bytes();
        // Junk (with a zlib stream of something else), then the pack in
        // chunks of 100 bytes, each stream at a multiple of 16.
        let mut archive = vec![0x5A; 0x103];
        archive.extend(zlib(b"not a pack"));
        for part in pack.chunks(100) {
            archive.resize(archive.len().next_multiple_of(16), 0);
            archive.extend(zlib(part));
        }
        archive.extend([0; 40]);
        assert_eq!(find(&archive).as_deref(), Some(&pack[..]));
        assert_eq!(find(&pack).as_deref(), Some(&pack[..]));
        assert!(find(&archive[..archive.len() - 200]).is_none());
    }

    #[test]
    fn rejects_bad_files() {
        let file = pack_bytes();
        assert_eq!(Pack::parse(b"SGD5").err(), Some("not a shader pack"));
        assert!(Pack::parse(&file[..file.len() - 1]).is_err());
        // A stage program index past the table.
        let mut bad = file.clone();
        bad[HEADER + 0x38..HEADER + 0x3c].copy_from_slice(&9_u32.to_le_bytes());
        assert_eq!(
            Pack::parse(&bad).err(),
            Some("graphics record refers outside the tables")
        );
        // A blob whose size runs past the blob data.
        let mut bad = file;
        let table = HEADER + GRAPHICS + COMPUTE;
        bad[table + 4..table + 8].copy_from_slice(&0xFFFF_u32.to_le_bytes());
        assert_eq!(Pack::parse(&bad).err(), Some("blob outside its data"));
    }
}
