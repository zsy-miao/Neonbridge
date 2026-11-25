//! Pipeline descriptions from the shader pack, as the engine hands them to
//! the device when it creates a pipeline at run time: its compact state
//! descriptions turned into D3D12 descriptions, in the creation stream the
//! engine builds (`victory-gpu` reads that stream, from the title or from
//! here, by the same code).
//!
//! The pack names each record's depth-stencil, rasteriser and blend state
//! only by CityHash64 of the engine's compact description (depth-stencil 24
//! bytes, rasteriser 16, blend 68). [`Named`] finds the descriptions behind
//! those hashes from the user's files: the shader databases' state blocks,
//! the states the pack's register images encode, and for rasterisers and
//! depth-stencil an enumeration of each field's usual values. A description
//! counts only if its hash is the record's. (Of 25,471 records these name
//! all three states of 21,036; one-byte changes of the named descriptions
//! and a blend enumeration named states of 17 and 32 more records, none
//! drawn with in the runs measured, and were left out.)
//!
//! Compact layouts (D3D12 enum values throughout):
//!
//! ```text
//! depth-stencil  b0 depth enable, b1 write mask, b2 func, b3 stencil enable,
//!                b4/b5 read/write mask, b6-9 front {fail, depth-fail, pass,
//!                func}, b10-13 back, b15 depth-bounds test
//! rasteriser     b0 fill, b1 cull, b2 front counter-clockwise, b3 i8 depth
//!                bias, +4 f32 bias clamp, +8 f32 slope bias, b12 depth clip,
//!                b14 multisample, b15 bit 0 antialiased lines, bit 1
//!                conservative
//! blend          b0 alpha to coverage, b1 independent, then from b4 eight
//!                targets {enable, src, dst, op, src alpha, dst alpha, op
//!                alpha, write mask}
//! ```

use std::collections::{HashMap, HashSet};

use crate::cityhash::city64;
use crate::sgd5::Database;
use crate::shaderpack::{Graphics, Pack};

pub const DEPTH: usize = 24;
pub const RASTER: usize = 16;
pub const BLEND: usize = 68;

/// A target as D3D12 defaults it: off, one, zero, add, write all.
const DEFAULT_TARGET: [u8; 8] = [0, 2, 1, 1, 2, 1, 1, 15];

/// The compact descriptions found for the pack's state hashes.
#[derive(Default)]
pub struct Named {
    pub depth: HashMap<u64, [u8; DEPTH]>,
    pub raster: HashMap<u64, [u8; RASTER]>,
    pub blend: HashMap<u64, [u8; BLEND]>,
    /// Where each was found: "database", "registers", "enumeration".
    pub source: HashMap<u64, &'static str>,
    finding: &'static str,
}

/// The hashes still to name, by kind.
struct Wanted {
    depth: HashSet<u64>,
    raster: HashSet<u64>,
    blend: HashSet<u64>,
}

impl Named {
    /// Names what it can of the state hashes `pack` uses, from `databases`
    /// and the pack itself.
    pub fn find(pack: &Pack, databases: &[Database]) -> Named {
        let mut want = Wanted {
            depth: pack.graphics().map(|g| g.depth_stencil_hash).collect(),
            raster: pack.graphics().map(|g| g.rasterizer_hash).collect(),
            blend: pack.graphics().map(|g| g.blend_hash).collect(),
        };
        let mut named = Named {
            finding: "database",
            ..Named::default()
        };
        for e in databases.iter().flat_map(|db| &db.effects) {
            e.depth_stencils
                .iter()
                .for_each(|b| named.try_depth(&mut want, b));
            e.rasterizers
                .iter()
                .for_each(|b| named.try_raster(&mut want, b));
            e.blends.iter().for_each(|b| named.try_blend(&mut want, b));
        }
        named.finding = "registers";
        named.name_from_registers(&mut want, pack);
        // Rasterisers have no register image here; depth states some
        // registers do not keep.
        named.finding = "enumeration";
        depth_space(|b| named.try_depth(&mut want, &b));
        raster_space(|b| named.try_raster(&mut want, &b));
        named
    }

    fn try_depth(&mut self, want: &mut Wanted, b: &[u8]) {
        try_one(&mut self.depth, &mut want.depth, b).map(|h| self.source.insert(h, self.finding));
    }

    fn try_raster(&mut self, want: &mut Wanted, b: &[u8]) {
        try_one(&mut self.raster, &mut want.raster, b).map(|h| self.source.insert(h, self.finding));
    }

    fn try_blend(&mut self, want: &mut Wanted, b: &[u8]) {
        try_one(&mut self.blend, &mut want.blend, b).map(|h| self.source.insert(h, self.finding));
    }

    /// The states the records' register images encode. The hardware does
    /// not keep every field (factors of a target that does not blend, the
    /// depth func with depth off): those are enumerated.
    fn name_from_registers(&mut self, want: &mut Wanted, pack: &Pack) {
        let mut tried = HashSet::new();
        for g in pack.graphics() {
            let (Some(regs), Some(formats)) = (registers(g.packet), g.target_formats()) else {
                continue;
            };
            if !tried.insert((g.depth_stencil_hash, g.blend_hash, formats, regs)) {
                continue;
            }
            if want.depth.contains(&g.depth_stencil_hash) {
                depth_from_registers(&regs, |b| self.try_depth(want, &b));
            }
            if want.blend.contains(&g.blend_hash) {
                blend_from_registers(&regs, &formats, |b| self.try_blend(want, &b));
            }
        }
    }
}

/// Names `b` if its hash is still wanted: the hash it names.
fn try_one<const N: usize>(
    named: &mut HashMap<u64, [u8; N]>,
    want: &mut HashSet<u64>,
    b: &[u8],
) -> Option<u64> {
    let b = <[u8; N]>::try_from(b).ok()?;
    let hash = city64(&b);
    want.remove(&hash)
        .then(|| named.insert(hash, b))
        .map(|_| hash)
}

/// Slope biases tried with each integer depth bias.
const SLOPES: [f32; 29] = [
    0.0, 0.5, 1.0, 1.5, 2.0, 2.5, 3.0, 4.0, 5.0, 8.0, 10.0, 16.0, 0.25, 0.1, 0.01, 0.001, -0.5,
    -1.0, -1.5, -2.0, -4.0, 0.75, 1.25, 6.0, 7.0, 20.0, 32.0, 64.0, 100.0,
];

fn raster_space(mut f: impl FnMut([u8; RASTER])) {
    let mut biases: Vec<i8> = (-16..=16).collect();
    biases.extend([-128, 127, 100, -100, 50, -50, 64, -64, 32, -32]);
    for fill in [2, 3] {
        for cull in [1, 2, 3] {
            for ccw in [0, 1] {
                for tail in 0..32u8 {
                    // Depth clip, the unused byte, multisample, line modes.
                    let tail = [tail & 1, tail >> 1 & 1, tail >> 2 & 1, tail >> 3];
                    let mut b = [0u8; RASTER];
                    b[..3].copy_from_slice(&[fill, cull, ccw]);
                    b[12..].copy_from_slice(&tail);
                    f(b);
                    for &bias in &biases {
                        for slope in SLOPES {
                            if bias == 0 && slope == 0.0 {
                                continue;
                            }
                            b[3] = bias as u8;
                            b[8..12].copy_from_slice(&slope.to_le_bytes());
                            f(b);
                        }
                    }
                }
            }
        }
    }
}

/// Stencil masks tried.
const MASKS: [u8; 19] = [
    0xFF, 0x0F, 0xF0, 0x01, 0x02, 0x04, 0x08, 0x10, 0x20, 0x40, 0x80, 0x03, 0x07, 0x0C, 0x30, 0xC0,
    0x00, 0x7F, 0xFE,
];

fn depth_space(mut f: impl FnMut([u8; DEPTH])) {
    for enable in [0, 1] {
        for write in [0, 1] {
            for func in 1..=8 {
                for tail in [[1, 1], [0, 0], [1, 0], [0, 1]] {
                    let mut b = [0u8; DEPTH];
                    b[..14].copy_from_slice(&[
                        enable, write, func, 0, 0xFF, 0xFF, 1, 1, 1, 8, 1, 1, 1, 8,
                    ]);
                    b[22..].copy_from_slice(&tail);
                    f(b);
                }
                // Stencil on, the same ops on both faces.
                for read in MASKS {
                    for written in MASKS {
                        for ops in 0..27u8 {
                            for sfunc in 1..=8 {
                                let face = [ops % 3 + 1, ops / 3 % 3 + 1, ops / 9 + 1, sfunc];
                                let mut b = [0u8; DEPTH];
                                b[..6].copy_from_slice(&[enable, write, func, 1, read, written]);
                                b[6..10].copy_from_slice(&face);
                                b[10..14].copy_from_slice(&face);
                                b[22..].copy_from_slice(&[1, 1]);
                                f(b);
                            }
                        }
                    }
                }
            }
        }
    }
}

/// The depth, stencil and blend registers of a pack record's register
/// image: `DB_DEPTH_CONTROL`, `DB_STENCIL_CONTROL`, `DB_STENCILREFMASK`
/// front and back, a word not used here, `CB_TARGET_MASK`, then
/// `CB_BLEND0..7_CONTROL`. They start 0x44 bytes into the fixed part, which
/// begins at 0x74 in vertex-shader images (dword 1 = 4) and 0xc8 in
/// geometry/mesh ones.
fn registers(packet: &[u8]) -> Option<[u32; 14]> {
    let word = |o: usize| Some(u32::from_le_bytes(packet.get(o..o + 4)?.try_into().ok()?));
    let base = if word(4)? == 4 { 0x74 } else { 0xc8 };
    let mut regs = [0; 14];
    for (i, r) in regs.iter_mut().enumerate() {
        *r = word(base + 0x44 + 4 * i)?;
    }
    Some(regs)
}

/// Hardware compare functions to D3D12's.
fn func(v: u32) -> u8 {
    (v & 7) as u8 + 1
}

/// Hardware stencil ops to D3D12's (unknown ones as keep).
fn stencil_op(v: u32) -> u8 {
    match v & 15 {
        0 => 1,
        1 => 2,
        3 | 4 => 3,
        5 => 4,
        6 => 5,
        7 => 6,
        8 => 7,
        9 => 8,
        _ => 1,
    }
}

fn depth_from_registers(regs: &[u32; 14], mut f: impl FnMut([u8; DEPTH])) {
    let [control, stencil, mask, ..] = *regs;
    let stencil_on = (control & 1) as u8;
    let depth_on = control >> 1 & 1 != 0;
    let off = [1, 1, 1, 8];
    let (front, back, masks) = if stencil_on != 0 {
        let front = [
            stencil_op(stencil),
            stencil_op(stencil >> 8),
            stencil_op(stencil >> 4),
            func(control >> 8),
        ];
        let back = if control >> 7 & 1 != 0 {
            [
                stencil_op(stencil >> 12),
                stencil_op(stencil >> 20),
                stencil_op(stencil >> 16),
                func(control >> 20),
            ]
        } else {
            front
        };
        (front, back, (mask >> 8 & 0xFF, mask >> 16 & 0xFF))
    } else {
        (off, off, (0xFF, 0xFF))
    };
    let (enables, writes, funcs): (&[u8], &[u8], Vec<u8>) = if depth_on {
        (&[1], &[(control >> 2 & 1) as u8], vec![func(control >> 4)])
    } else {
        (&[0, 1], &[0, 1], (1..=8).collect())
    };
    let mut faces = vec![(front, back)];
    if stencil_on != 0 {
        // The second face when back-face stencil is off.
        faces.push((front, off));
    }
    for &e in enables {
        for &w in writes {
            for &fun in &funcs {
                for &(front, back) in &faces {
                    let mut b = [0u8; DEPTH];
                    b[..6].copy_from_slice(&[e, w, fun, stencil_on, masks.0 as u8, masks.1 as u8]);
                    b[6..10].copy_from_slice(&front);
                    b[10..14].copy_from_slice(&back);
                    b[22..].copy_from_slice(&[1, 1]);
                    f(b);
                    if stencil_on == 0 {
                        b[4] = (mask >> 8) as u8;
                        b[5] = (mask >> 16) as u8;
                        f(b);
                    }
                }
            }
        }
    }
}

/// Hardware blend factors to D3D12's.
fn factor(v: u32) -> Option<u8> {
    match v & 31 {
        v @ 0..=10 => Some(v as u8 + 1),
        v @ 13..=20 => Some(v as u8 + 1),
        _ => None,
    }
}

/// Hardware blend functions to D3D12's.
fn combine(v: u32) -> Option<u8> {
    match v & 7 {
        0 => Some(1),
        1 => Some(2),
        4 => Some(3),
        2 => Some(4),
        3 => Some(5),
        _ => None,
    }
}

/// Candidate compact entries for one target from its blend control.
fn blend_target(control: u32, mask: u8) -> Vec<[u8; 8]> {
    if control >> 30 & 1 == 0 {
        // Off: the factors are not kept; the defaults and the common ones.
        return [
            [2, 1, 1, 2, 1, 1],
            [5, 6, 1, 2, 6, 1],
            [5, 6, 1, 5, 6, 1],
            [2, 2, 1, 2, 2, 1],
            [2, 6, 1, 2, 6, 1],
            [1, 1, 1, 1, 1, 1],
        ]
        .map(|f| [0, f[0], f[1], f[2], f[3], f[4], f[5], mask])
        .to_vec();
    }
    let color = (factor(control), combine(control >> 5), factor(control >> 8));
    let alpha = if control >> 29 & 1 != 0 {
        (
            factor(control >> 16),
            combine(control >> 21),
            factor(control >> 24),
        )
    } else {
        color
    };
    let ((Some(s), Some(op), Some(d)), (Some(sa), Some(opa), Some(da))) = (color, alpha) else {
        return Vec::new();
    };
    // D3D12 names the alpha slots' colour factors as their alpha twins.
    let twins = |f: u8| {
        let alias = [(3, 5), (4, 6), (9, 7), (10, 8), (16, 18), (17, 19)];
        std::iter::once(f)
            .chain(alias.iter().filter(|a| a.1 == f).map(|a| a.0))
            .collect::<Vec<u8>>()
    };
    let mut out = Vec::new();
    for sa in twins(sa) {
        for &da in &twins(da) {
            out.push([1, s, d, op, sa, da, opa, mask]);
        }
    }
    out
}

fn blend_from_registers(regs: &[u32; 14], formats: &[u8; 8], mut f: impl FnMut([u8; BLEND])) {
    let target_mask = regs[5];
    let per_target: Vec<Vec<[u8; 8]>> = (0..8)
        .map(|i| {
            let control = regs[6 + i];
            let m = (target_mask >> (4 * i) & 15) as u8;
            let mut masks = vec![m];
            if formats[i] == 0 {
                masks.extend([0, 15]);
                if i > 0 {
                    masks.push((target_mask & 15) as u8);
                }
                masks.sort_unstable();
                masks.dedup();
            }
            let mut c: Vec<[u8; 8]> = masks
                .iter()
                .flat_map(|&m| blend_target(control, m))
                .collect();
            if formats[i] == 0 {
                let mut cleared = DEFAULT_TARGET;
                cleared[7] = 0;
                c.extend([DEFAULT_TARGET, cleared]);
            }
            c
        })
        .collect();
    if per_target.iter().any(Vec::is_empty) {
        return;
    }
    let build = |head: [u8; 4], targets: &[[u8; 8]]| {
        let mut b = [0u8; BLEND];
        b[..4].copy_from_slice(&head);
        for (i, t) in targets.iter().enumerate() {
            b[4 + 8 * i..12 + 8 * i].copy_from_slice(t);
        }
        b
    };
    for independent in [0, 1] {
        for a2c in [0, 1] {
            let head = [a2c, independent, 0, 0];
            if independent == 1 {
                // Each target's first reading, and with the targets that
                // have no format (whose registers repeat another target's)
                // left at D3D12's default, writing nothing up to some target
                // and everything from there; then one target changed.
                let first: Vec<[u8; 8]> = per_target.iter().map(|c| c[0]).collect();
                let mut bases = vec![first.clone()];
                for split in 0..=8 {
                    let mut base = first.clone();
                    for (i, t) in base
                        .iter_mut()
                        .enumerate()
                        .filter(|(i, _)| formats[*i] == 0)
                    {
                        *t = DEFAULT_TARGET;
                        t[7] = if i < split { 0 } else { 15 };
                    }
                    bases.push(base);
                }
                for base in &bases {
                    f(build(head, base));
                    for (i, c) in per_target.iter().enumerate() {
                        for &t in c {
                            let mut targets = base.clone();
                            targets[i] = t;
                            f(build(head, &targets));
                        }
                    }
                }
            } else {
                for &c0 in &per_target[0] {
                    f(build(head, &[c0; 8]));
                    let mut keep_off = c0;
                    keep_off[7] = 0;
                    let mut default_off = DEFAULT_TARGET;
                    default_off[7] = 0;
                    let mut off = c0;
                    off[0] = 0;
                    off[7] = 0;
                    let mut disabled = c0;
                    disabled[0] = 0;
                    for fill in [DEFAULT_TARGET, keep_off, default_off, [0; 8], off, disabled] {
                        for n in 1..8 {
                            let targets: Vec<[u8; 8]> =
                                (0..8).map(|i| if i < n { c0 } else { fill }).collect();
                            f(build(head, &targets));
                        }
                    }
                }
            }
        }
    }
}

/// The engine's D3D12 depth-stencil description (the form with the
/// depth-bounds test) of a compact one: no depth writes without the depth
/// test, no stencil writes unless some stencil op can write.
pub fn depth_stencil_desc(c: &[u8; DEPTH]) -> [u8; 0x38] {
    let mut d = [0u8; 0x38];
    let mut put = |at: usize, v: u8| d[at..at + 4].copy_from_slice(&u32::from(v).to_le_bytes());
    put(0, c[0]);
    put(4, if c[0] != 0 { c[1] } else { 0 });
    put(8, c[2]);
    put(0xc, c[3]);
    for i in 0..8 {
        put(0x14 + 4 * i, c[6 + i]);
    }
    put(0x34, c[15]);
    d[0x10] = c[4];
    d[0x11] = if stencil_writes(c) { c[5] } else { 0 };
    d
}

/// Whether the stencil can be written (stencil on, a write mask, and an op
/// on some face that writes for some pixel).
fn stencil_writes(c: &[u8; DEPTH]) -> bool {
    let face = |f: &[u8]| {
        if (f[3] == 1 || f[2] == 1) && (f[3] == 8 || f[0] == 1) {
            f[1] != 1 && c[0] & 1 != 0 && c[2] != 8
        } else {
            true
        }
    };
    c[3] == 1 && c[5] != 0 && (face(&c[6..10]) || face(&c[10..14]))
}

/// The engine's D3D12 rasteriser description of a compact one.
pub fn rasterizer_desc(c: &[u8; RASTER]) -> [u8; 0x2c] {
    let mut d = [0u8; 0x2c];
    let words = [
        u32::from(c[0]),
        u32::from(c[1]),
        u32::from(c[2]),
        c[3] as i8 as i32 as u32,
        u32::from_le_bytes(c[4..8].try_into().expect("4")),
        u32::from_le_bytes(c[8..12].try_into().expect("4")),
        u32::from(c[12]),
        u32::from(c[14]),
        u32::from(c[15] & 1),
        0,
        u32::from(c[15] >> 1 & 1),
    ];
    for (i, w) in words.iter().enumerate() {
        d[4 * i..4 * i + 4].copy_from_slice(&w.to_le_bytes());
    }
    d
}

/// The engine's D3D12 blend description of a compact one: no logic ops
/// (each target's logic op set to no-op).
pub fn blend_desc(c: &[u8; BLEND]) -> [u8; 0x148] {
    let mut d = [0u8; 0x148];
    d[0] = c[0];
    d[4] = c[1];
    for i in 0..8 {
        let (t, at) = (&c[4 + 8 * i..12 + 8 * i], 8 + 40 * i);
        let words = [t[0], 0, t[1], t[2], t[3], t[4], t[5], t[6], 4];
        for (k, &w) in words.iter().enumerate() {
            d[at + 4 * k] = w;
        }
        d[at + 36] = t[7];
    }
    d
}

/// Why a pack record gives no creation stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Unnamed {
    DepthStencil,
    Rasterizer,
    Blend,
    /// The metadata does not read as a description.
    Metadata,
}

/// A creation stream as the engine builds it (0x2b0 bytes): root signature,
/// five shader slots (the first a vertex or mesh shader), blend, sample
/// mask, rasteriser, depth-stencil (with depth bounds), sample description,
/// input layout, topology, target formats, depth format. Its pointers go
/// to the shaders passed in and to the input elements it holds.
pub struct Stream<'a> {
    bytes: Vec<u8>,
    _elements: Vec<[u8; 32]>,
    _names: Vec<Vec<u8>>,
    _shaders: std::marker::PhantomData<&'a [u8]>,
}

impl Stream<'_> {
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

/// The stream the engine builds for record `g` with the given shader
/// containers (`first` a mesh shader if `mesh`).
pub fn stream<'a>(
    g: &Graphics,
    named: &Named,
    first: &'a [u8],
    mesh: bool,
    pixel: Option<&'a [u8]>,
) -> Result<Stream<'a>, Unnamed> {
    let depth = named
        .depth
        .get(&g.depth_stencil_hash)
        .ok_or(Unnamed::DepthStencil)?;
    let raster = named
        .raster
        .get(&g.rasterizer_hash)
        .ok_or(Unnamed::Rasterizer)?;
    let blend = named.blend.get(&g.blend_hash).ok_or(Unnamed::Blend)?;
    let (Some(formats), Some((depth_format, _, topology)), Some((_, count, quality)), Some(inputs)) = (
        g.target_formats(),
        g.depth_targets_topology(),
        g.samples(),
        g.input_layout(),
    ) else {
        return Err(Unnamed::Metadata);
    };
    let mut s = vec![0u8; 0x2b0];
    let put = |s: &mut Vec<u8>, at: usize, v: u32| s[at..at + 4].copy_from_slice(&v.to_le_bytes());
    let put64 =
        |s: &mut Vec<u8>, at: usize, v: u64| s[at..at + 8].copy_from_slice(&v.to_le_bytes());
    for (at, kind) in [
        (0x10, if mesh { 24 } else { 1 }),
        (0x28, 2),
        (0x40, 3),
        (0x58, 4),
        (0x70, 5),
        (0x88, 8),
        (0x1d8, 9),
        (0x1e0, 10),
        (0x210, 21),
        (0x250, 17),
        (0x260, 12),
        (0x278, 14),
        (0x280, 15),
        (0x2a8, 16),
    ] {
        put(&mut s, at, kind);
    }
    put64(&mut s, 0x18, first.as_ptr() as u64);
    put64(&mut s, 0x20, first.len() as u64);
    if let Some(ps) = pixel {
        put64(&mut s, 0x30, ps.as_ptr() as u64);
        put64(&mut s, 0x38, ps.len() as u64);
    }
    s[0x8c..0x1d4].copy_from_slice(&blend_desc(blend));
    put(&mut s, 0x1dc, u32::MAX);
    s[0x1e4..0x210].copy_from_slice(&rasterizer_desc(raster));
    s[0x214..0x24c].copy_from_slice(&depth_stencil_desc(depth));
    // The engine makes 1 to 16 samples, a power of two; 21 records store
    // other values there, and a run creates those pipelines with one.
    let (count, quality) = if [1, 2, 4, 8, 16].contains(&count) {
        (count, quality)
    } else {
        (1, 0)
    };
    put(&mut s, 0x254, count);
    put(&mut s, 0x258, quality);
    let names: Vec<Vec<u8>> = inputs
        .iter()
        .map(|e| [e.semantic.as_bytes(), &[0]].concat())
        .collect();
    let elements: Vec<[u8; 32]> = inputs
        .iter()
        .zip(&names)
        .map(|(e, name)| {
            let mut b = [0u8; 32];
            b[..8].copy_from_slice(&(name.as_ptr() as u64).to_le_bytes());
            for (i, w) in [e.index, e.format, e.slot, e.offset, e.class, e.step_rate]
                .iter()
                .enumerate()
            {
                b[8 + 4 * i..12 + 4 * i].copy_from_slice(&w.to_le_bytes());
            }
            b
        })
        .collect();
    if !elements.is_empty() {
        put64(&mut s, 0x268, elements.as_ptr() as u64);
        put(&mut s, 0x270, elements.len() as u32);
    }
    put(&mut s, 0x27c, u32::from(topology));
    for (i, &f) in formats.iter().enumerate() {
        put(&mut s, 0x284 + 4 * i, u32::from(f));
    }
    // The engine counts targets up to the last one with a format.
    let targets = formats.iter().rposition(|&f| f != 0).map_or(0, |i| i + 1);
    put(&mut s, 0x2a4, targets as u32);
    put(&mut s, 0x2ac, u32::from(depth_format));
    Ok(Stream {
        bytes: s,
        _elements: elements,
        _names: names,
        _shaders: std::marker::PhantomData,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn depth_writes_need_the_test_and_a_writing_op() {
        // Depth off, write on; stencil on with keep everywhere.
        let mut c = [0u8; DEPTH];
        c[..14].copy_from_slice(&[0, 1, 4, 1, 0xF0, 0x0F, 1, 1, 1, 8, 1, 1, 1, 8]);
        let d = depth_stencil_desc(&c);
        let word = |at: usize| u32::from_le_bytes(d[at..at + 4].try_into().unwrap());
        assert_eq!((word(0), word(4), word(8), word(0xc)), (0, 0, 4, 1));
        assert_eq!(
            (d[0x10], d[0x11]),
            (0xF0, 0),
            "keep everywhere writes no stencil"
        );
        // A replace on pass writes.
        c[8] = 3;
        assert_eq!(depth_stencil_desc(&c)[0x11], 0x0F);
        // Keep on pass and fail, func always: only depth-fail could write,
        // and with the depth test off it never runs.
        c[8] = 1;
        c[7] = 2;
        assert_eq!(depth_stencil_desc(&c)[0x11], 0);
        c[0] = 1;
        let d = depth_stencil_desc(&c);
        assert_eq!((d[4], d[0x11]), (1, 0x0F));
    }

    #[test]
    fn rasterizer_fields_go_where_d3d12_has_them() {
        let mut c = [0u8; RASTER];
        c[..4].copy_from_slice(&[3, 2, 1, 0xFE]);
        c[8..12].copy_from_slice(&1.5f32.to_le_bytes());
        c[12..].copy_from_slice(&[1, 1, 1, 3]);
        let d = rasterizer_desc(&c);
        let w: Vec<u32> = d
            .chunks(4)
            .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
            .collect();
        assert_eq!(
            w,
            [3, 2, 1, (-2i32) as u32, 0, 1.5f32.to_bits(), 1, 1, 1, 0, 1]
        );
    }

    #[test]
    fn blend_targets_get_no_logic_op() {
        let mut c = [0u8; BLEND];
        c[..2].copy_from_slice(&[1, 0]);
        c[4..12].copy_from_slice(&[1, 5, 6, 1, 2, 1, 1, 7]);
        let d = blend_desc(&c);
        let w = |at: usize| u32::from_le_bytes(d[at..at + 4].try_into().unwrap());
        assert_eq!((w(0), w(4)), (1, 0));
        let target: Vec<u32> = (0..9).map(|k| w(8 + 4 * k)).collect();
        assert_eq!(target, [1, 0, 5, 6, 1, 2, 1, 1, 4]);
        assert_eq!(d[8 + 36], 7);
        assert_eq!(w(48 + 32), 4, "second target's logic op");
    }

    #[test]
    fn the_depth_space_holds_the_d3d12_default() {
        let mut found = false;
        depth_space(|b| found |= b[..6] == [1, 1, 2, 0, 0xFF, 0xFF] && b[22..] == [1, 1]);
        assert!(found);
    }

    /// A record's metadata: targets 28 and 10, depth format 40, topology
    /// 3, two samples, and a layout of POSITION and COLOR.
    fn metadata() -> Vec<u8> {
        let mut m = vec![0; 0x4c];
        m[0x10..0x18].copy_from_slice(&[28, 0, 10, 0, 0, 0, 0, 0]);
        m[0x1c..0x20].copy_from_slice(&u32::MAX.to_le_bytes());
        m[0x24..0x27].copy_from_slice(&[40, 3, 3]);
        m[0x28] = 2;
        let mut layout = 2u32.to_le_bytes().to_vec();
        for (name, words) in [
            ("POSITION", [0u32, 6, 0, 0, 0, 0]),
            ("COLOR", [1, 28, 1, 12, 1, 1]),
        ] {
            layout.extend((name.len() as u32 + 1).to_le_bytes());
            layout.extend(name.bytes().chain([0]));
            words.iter().for_each(|w| layout.extend(w.to_le_bytes()));
        }
        m[0x38..0x3c].copy_from_slice(&(layout.len() as u32).to_le_bytes());
        m.extend(layout);
        m
    }

    #[test]
    fn a_record_becomes_the_engines_creation_stream() {
        let meta = metadata();
        let g = Graphics {
            key: 0,
            shader_hash: 0,
            pipeline_hash: 0,
            depth_stencil_hash: 1,
            rasterizer_hash: 2,
            blend_hash: 3,
            packet: &[],
            metadata: &meta,
            stages: [None; 7],
        };
        let mut named = Named::default();
        let mut depth = [0u8; DEPTH];
        depth[..3].copy_from_slice(&[1, 1, 4]);
        named.depth.insert(1, depth);
        named
            .raster
            .insert(2, [3, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 0, 0]);
        let (vs, ps) = ([1u8; 12], [2u8; 20]);
        assert_eq!(
            stream(&g, &named, &vs, false, Some(&ps)).err(),
            Some(Unnamed::Blend)
        );
        let mut blend = [0u8; BLEND];
        blend[4..12].copy_from_slice(&[1, 5, 6, 1, 2, 1, 1, 15]);
        named.blend.insert(3, blend);

        let s = stream(&g, &named, &vs, false, Some(&ps)).unwrap();
        let b = s.bytes();
        let w = |at: usize| u32::from_le_bytes(b[at..at + 4].try_into().unwrap());
        let q = |at: usize| u64::from_le_bytes(b[at..at + 8].try_into().unwrap());
        assert_eq!(b.len(), 0x2b0);
        assert_eq!((w(0x10), q(0x18), q(0x20)), (1, vs.as_ptr() as u64, 12));
        assert_eq!((w(0x28), q(0x30), q(0x38)), (2, ps.as_ptr() as u64, 20));
        assert_eq!(b[0x8c..0x1d4], blend_desc(&blend));
        assert_eq!(w(0x1dc), u32::MAX);
        assert_eq!(b[0x1e4..0x210], rasterizer_desc(&named.raster[&2]));
        assert_eq!(
            (w(0x210), b[0x214..0x24c].to_vec()),
            (21, depth_stencil_desc(&depth).to_vec())
        );
        assert_eq!((w(0x254), w(0x258), w(0x270), w(0x27c)), (2, 0, 2, 3));
        // Target 1 has no format, target 2 has: three targets.
        assert_eq!(
            (w(0x284), w(0x288), w(0x28c), w(0x2a4), w(0x2ac)),
            (28, 0, 10, 3, 40)
        );
        // The second input element, through the stream's pointer.
        // SAFETY: the stream holds its elements and their names.
        let (name, words) = unsafe {
            let e = (q(0x268) as *const [u8; 32]).add(1).read();
            let name = std::ffi::CStr::from_ptr(
                u64::from_le_bytes(e[..8].try_into().unwrap()) as *const _
            );
            (name.to_str().unwrap().to_owned(), e[8..].to_vec())
        };
        assert_eq!(name, "COLOR");
        let words: Vec<u32> = words
            .chunks(4)
            .map(|c| u32::from_le_bytes(c.try_into().unwrap()))
            .collect();
        assert_eq!(words, [1, 28, 1, 12, 1, 1]);
        // A mesh shader goes in the first slot as type 24.
        let s = stream(&g, &named, &vs, true, None).unwrap();
        assert_eq!(
            (s.bytes()[0x10], &s.bytes()[0x30..0x40]),
            (24, &[0u8; 16][..])
        );
    }

    #[test]
    fn registers_name_a_blend() {
        // Target 0: src alpha (4), inverse src alpha (5), add; others off.
        let mut regs = [0u32; 14];
        regs[5] = 0xF; // target mask: target 0 writes all
        regs[6] = 1 << 30 | 4 | 5 << 8;
        let want = {
            let mut b = [0u8; BLEND];
            b[4..12].copy_from_slice(&[1, 5, 6, 1, 5, 6, 1, 15]);
            for i in 1..8 {
                b[4 + 8 * i..12 + 8 * i].copy_from_slice(&DEFAULT_TARGET);
            }
            b
        };
        let mut found = false;
        blend_from_registers(&regs, &[28, 0, 0, 0, 0, 0, 0, 0], |b| found |= b == want);
        assert!(found);
    }

    #[test]
    fn registers_name_an_independent_blend() {
        // Target 0 blends; the targets without a format repeat its
        // control, and the description leaves them at D3D12's default,
        // writing nothing for targets 1-2 and everything from 3 on.
        let mut regs = [0u32; 14];
        regs[5] = 0xF;
        regs[6..].fill(1 << 30 | 4 | 5 << 8);
        let mut want = [0u8; BLEND];
        want[1] = 1;
        want[4..12].copy_from_slice(&[1, 5, 6, 1, 5, 6, 1, 15]);
        for i in 1..8 {
            want[4 + 8 * i..12 + 8 * i].copy_from_slice(&DEFAULT_TARGET);
            if i < 3 {
                want[11 + 8 * i] = 0;
            }
        }
        let mut found = false;
        blend_from_registers(&regs, &[28, 0, 0, 0, 0, 0, 0, 0], |b| found |= b == want);
        assert!(found);
    }
}
