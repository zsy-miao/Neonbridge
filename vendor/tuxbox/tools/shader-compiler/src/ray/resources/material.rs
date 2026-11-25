//! Local shader records and descriptor waterfalls in triangle hit programs.
//! Descriptor addresses below are internal values. The host reads the actual
//! root arguments associated with this hit record for each invocation.
mod image;
mod relative;
use super::{Bindings, local_record};
use crate::ray::alu::{Mul24, SDWA_CLAMP, sdwa_field, whole_sdwa_words};
use crate::rdna2::{Enc, Insn, Opnd, instructions};
use std::{collections::BTreeMap, fmt::Write, ops::Range};
use tuxbox_dxbc::ray::State;

const SRV: u32 = 0x1000_0000;
const CBV: u32 = 0x2000_0000;
const SAMPLER: u32 = 0x4000_0000;
const BVH_BASE: u32 = 0x3000_0000;
const TRIANGLE_BASE: u32 = 0x4000_0000;

const GEOMETRY: &str = r#"
ByteAddressBuffer tbIndices : register(t0,space2);
ByteAddressBuffer tbVertices : register(t1,space2);
cbuffer TriangleParameters : register(b0,space2) { uint4 tbGeometry; };
"#;

fn index(op: &Opnd) -> Option<usize> {
    match op {
        Opnd::Sgpr(n) if *n < 128 => Some(*n as usize),
        Opnd::Vgpr(n) if *n < 128 => Some(128 + *n as usize),
        _ => None,
    }
}

#[derive(Clone)]
struct Values {
    words: [Option<u32>; 384],
    code_address: [bool; 384],
}

impl Values {
    fn new() -> Self {
        let mut words = [None; 384];
        for (i, v) in [
            (8, 0),
            (128 + 5, 0),
            (128 + 2, 8),
            (128 + 3, 8),
            (128 + 16, BVH_BASE >> 8),
            (128 + 17, 0),
        ] {
            words[i] = Some(v);
        }
        Self {
            words,
            code_address: [false; 384],
        }
    }

    fn get(&self, op: &Opnd) -> Option<u32> {
        match op {
            Opnd::ConstInt(x) => Some(*x as u32),
            Opnd::Literal(x) => Some(*x),
            _ => self.words[index(op)?],
        }
    }

    fn pair(&self, op: &Opnd) -> Option<u64> {
        let k = index(op)?;
        Some(u64::from(self.words[k]?) | (u64::from(self.words[k + 1]?) << 32))
    }

    fn sdwa_alu(&self, ins: &Insn, ext: u32) -> Option<u32> {
        if ins.enc != Enc::Vop2 || ext & 0x7070_0000 != 0 {
            return None;
        }
        let read = |op: &Opnd, field: u32| {
            let word = match op {
                Opnd::Literal(_) => return None, // SDWA has no trailing literal word
                Opnd::ConstF32(f) => f.to_bits(),
                Opnd::Null => 0,
                _ => self.get(op)?,
            };
            let (width, offset) = sdwa_field(field & 7)?;
            let aligned = word << (32 - width - offset);
            Some(if field & 8 == 0 {
                aligned >> (32 - width)
            } else {
                ((aligned as i32) >> (32 - width)) as u32
            })
        };
        let a = read(ins.src.first()?, ext >> 16)?;
        let b = read(ins.src.get(1)?, ext >> 24)?;
        let mut control = (ext >> 8) & 255;
        let value = match ins.mnemonic.as_str() {
            "v_min_i32" => (a as i32).min(b as i32) as u32,
            "v_max_i32" => (a as i32).max(b as i32) as u32,
            "v_min_u32" => a.min(b),
            "v_max_u32" => a.max(b),
            "v_lshlrev_b32" => b.wrapping_shl(a),
            "v_lshrrev_b32" => b.wrapping_shr(a),
            "v_ashrrev_i32" => (b as i32).wrapping_shr(a) as u32,
            "v_add_nc_u32" => a.wrapping_add(b),
            "v_sub_nc_u32" => a.wrapping_sub(b),
            "v_and_b32" => a & b,
            "v_or_b32" => a | b,
            "v_xor_b32" => a ^ b,
            _ => {
                let mul = Mul24::decode(ins)?;
                control &= !SDWA_CLAMP;
                mul.fold(a, b)
            }
        };
        let (width, offset) = sdwa_field((ext >> 8) & 7)?;
        let mask = u32::MAX >> (32 - width);
        let placed = (value & mask) << offset;
        match control >> 3 {
            0 => Some(placed),
            1 => Some((((value << (32 - width)) as i32) >> (32 - width - offset)) as u32),
            2 if width == 32 => Some(value),
            2 => Some((self.get(ins.dst.first()?)? & !(mask << offset)) | placed),
            _ => None,
        }
    }

    fn alu(&self, ins: &Insn) -> [Option<u32>; 2] {
        if let Some(ext) = ins.aux.sdwa {
            let value = self.sdwa_alu(ins, ext);
            // Identity selectors also allow existing whole-word folds, e.g. MOV.
            if value.is_some() || !whole_sdwa_words(ins)
                || ins.src.iter().any(|op| matches!(op, Opnd::Literal(_)))
            {
                return [value, None];
            }
        }
        let x = ins.src.first().and_then(|x| self.get(x));
        let y = ins.src.get(1).and_then(|x| self.get(x));
        let z = ins.src.get(2).and_then(|x| self.get(x));
        if let Some(mul) = Mul24::decode(ins) {
            return [x.zip(y).map(|(a, b)| mul.fold(a, b)), None];
        }
        if ins.mnemonic == "s_getpc_b64" {
            return [
                Some(super::super::continuation::CODE_BASE + (ins.offset + ins.dwords * 4) as u32),
                Some(0),
            ];
        }
        let pair = match ins.mnemonic.as_str() {
            "s_lshr_b64" | "s_lshl_b64" => {
                ins.src
                    .first()
                    .and_then(|op| self.pair(op))
                    .zip(y)
                    .map(|(x, s)| {
                        if ins.mnemonic == "s_lshl_b64" {
                            x.wrapping_shl(s)
                        } else {
                            x.wrapping_shr(s)
                        }
                    })
            }
            "v_lshlrev_b64" => ins
                .src
                .get(1)
                .and_then(|op| self.pair(op))
                .zip(x)
                .map(|(y, s)| y.wrapping_shl(s)),
            _ => None,
        };
        if let Some(pair) = pair {
            return [Some(pair as u32), Some((pair >> 32) as u32)];
        }
        let value = match ins.mnemonic.as_str() {
            "v_mov_b32" | "s_mov_b32" | "v_readfirstlane_b32" => x,
            "v_add_nc_u32" | "v_add_co_u32" | "s_add_u32" => {
                x.zip(y).map(|(x, y)| x.wrapping_add(y))
            }
            // Local tables use zero-based offsets smaller than 4 GiB. Carry
            // cannot cross the high dword while indexing one of these tables.
            "v_add_co_ci_u32" | "s_addc_u32" => x.zip(y).map(|(x, y)| x.wrapping_add(y)),
            "s_lshl_b32" => x.zip(y).map(|(x, y)| x.wrapping_shl(y)),
            "v_or_b32" => x.zip(y).map(|(x, y)| x | y),
            "v_and_b32" => x.zip(y).map(|(x, y)| x & y),
            "v_lshrrev_b32" => x.zip(y).map(|(x, y)| y.wrapping_shr(x)),
            "v_lshl_add_u32" => x
                .zip(y)
                .zip(z)
                .map(|((x, y), z)| x.wrapping_shl(y).wrapping_add(z)),
            "v_cndmask_b32" if x == y => x,
            _ => None,
        };
        [value, None]
    }
}

enum Word {
    Immediate(u32),
    Value(&'static str),
    InverseTransform(u32, u32),
}

fn local_word(address: u32) -> Option<Word> {
    // The virtual nodes have one 64-byte unit of offset, like v2/v3 at
    // entry. The native shader uses node+48, +60, +112 and triangle+52.
    Some(match address {
        32 | 40 | 68 | 76 | 84 => Word::Immediate(0),
        36 => Word::Immediate(1),
        44 => Word::Immediate(2),
        48 => Word::Value("tbGeometry.x"),
        52 => Word::Value("tbGeometry.y"),
        56 => Word::Value("tbGeometry.z"),
        60 => Word::Value("tbGeometry.w"),
        64 => Word::Immediate(SRV),
        72 => Word::Immediate(CBV),
        80 => Word::Immediate(SAMPLER),
        0x3000_0070 => Word::Value("InstanceID()"),
        0x3000_007c => Word::Immediate(TRIANGLE_BASE >> 8),
        // The sideband matrix lays its three translation words before
        // the row-major inverse linear transform. The hit shader multiplies
        // the object-space face normal by its transpose. DXR exposes the
        // uncompressed world-to-object transform for the intersected instance.
        0x3000_0080..=0x3000_0088 if address.is_multiple_of(4) => {
            Word::InverseTransform((address - 0x3000_0080) / 4, 3)
        }
        0x3000_008c..=0x3000_00ac if address.is_multiple_of(4) => {
            let element = (address - 0x3000_008c) / 4;
            Word::InverseTransform(element / 3, element % 3)
        }
        0x3000_00b0 => Word::Value("InstanceIndex()"),
        0x3000_00b8 => Word::Immediate(0), // uncompressed 3x3 matrix
        0x4000_0074 => Word::Value("PrimitiveIndex()"),
        _ => return None,
    })
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum View {
    Structured,
    Typed,
    Constant,
    Page,
    Texture,
    Volume,
    Cube,
    Array,
    Sampler,
}

#[derive(Default)]
struct Layout {
    constant_addresses: Range<u32>,
    relative_writes: BTreeMap<usize, Range<usize>>,
    views: BTreeMap<(u32, u32), View>,
    emitted: BTreeMap<usize, String>,
    // Known VGPR contents saved at fixed offsets in the compiler-private frame.
    spills: BTreeMap<u32, (Option<u32>, bool)>,
}

impl Layout {
    fn resource(&mut self, value: u32, bank: u32, kind: View) -> Option<String> {
        let stride = if bank == SAMPLER { 16 } else { 32 };
        let offset = value.checked_sub(bank)?;
        let slot = offset / stride;
        if !offset.is_multiple_of(stride) || slot >= if bank == SRV { 64 } else { 16 } {
            return None;
        }
        if self
            .views
            .insert((bank, slot), kind)
            .is_some_and(|old| old != kind)
        {
            return None;
        }
        Some(format!(
            "tb{}{}",
            match bank {
                SRV => "Srv",
                CBV => "Cb",
                SAMPLER => "Sampler",
                _ => return None,
            },
            slot
        ))
    }

    fn buffer(&mut self, ins: &Insn, regs: &Values, dst: u8) -> Option<String> {
        let [Opnd::Vgpr(at), base, Opnd::ConstInt(0)] = ins.src.as_slice() else {
            return None;
        };
        let pointer = regs.get(base)?;
        let count = match ins.op {
            0..=3 => 1 + ins.op,
            12 => 1,
            13 => 2,
            14 => 4,
            15 => 3,
            _ => return None,
        };
        let constant = (CBV..CBV + 512).contains(&pointer);
        let kind = if constant {
            View::Constant
        } else if ins.op < 4 {
            View::Typed
        } else {
            View::Structured
        };
        let name = self.resource(pointer, if constant { CBV } else { SRV }, kind)?;
        let offset = u32::try_from(ins.aux.imm).ok()?;
        if !offset.is_multiple_of(4) || offset / 4 + count > 4 {
            return None;
        }
        let element = match ins.aux.mask {
            0 => "0".to_string(),
            2 => format!("v[{at}]"),
            _ => return None,
        };
        let mut body = format!("{{ uint4 words=asuint({name}[{element}]);");
        for i in 0..count {
            write!(
                body,
                " v[{}]=words[{}];",
                u32::from(dst) + i,
                offset / 4 + i
            )
            .ok()?;
        }
        body.push_str(" }");
        Some(body)
    }

    fn instruction(&mut self, ins: &Insn, regs: &mut Values) -> Option<()> {
        let dst = ins.dst.first().and_then(index);
        match ins.mnemonic.as_str() {
            "v_swap_b32" => {
                let source = index(ins.src.first()?)?;
                regs.words.swap(dst?, source);
                regs.code_address.swap(dst?, source);
                return Some(());
            }
            "v_movreld_b32" => {
                // A runtime M0 can select any physical VGPR after wrapping.
                let range = self
                    .relative_writes
                    .get(&ins.offset)
                    .cloned()
                    .unwrap_or(0..256);
                regs.words[128 + range.start..128 + range.end].fill(None);
                regs.code_address[128 + range.start..128 + range.end].fill(false);
                return Some(());
            }
            _ => {}
        }

        let from_code = |operand: &Opnd| index(operand).is_some_and(|reg| regs.code_address[reg]);
        let is_table_pointer = match ins.mnemonic.as_str() {
            "s_getpc_b64" => !self.constant_addresses.is_empty(),
            "v_add_co_u32" | "v_add_nc_u32" | "s_add_u32" => ins.src.iter().take(2).any(from_code),
            "v_mov_b32" | "s_mov_b32" | "v_readfirstlane_b32" => {
                ins.src.first().is_some_and(from_code)
            }
            _ => false,
        };

        let dw = match ins.dst.first() {
            Some(Opnd::Sgpr(x) | Opnd::Vgpr(x)) => *x,
            _ => 0,
        };
        if ins
            .src
            .iter()
            .chain(&ins.dst)
            .any(|op| matches!(op, Opnd::Vgpr(128..=255) | Opnd::Sgpr(128..=255)))
        {
            return None;
        }
        if super::super::continuation::scratch(ins)
            && ins.aux.mask == 0
            && matches!(ins.op, 12 | 28)
        {
            let addr = u32::try_from(ins.aux.imm).ok()?;
            if !addr.is_multiple_of(4) {
                return None;
            }
            let k = dst?;
            if ins.op == 28 {
                self.spills
                    .insert(addr, (regs.words[k], regs.code_address[k]));
            } else {
                (regs.words[k], regs.code_address[k]) =
                    self.spills.get(&addr).copied().unwrap_or_default();
            }
            return Some(());
        }
        let mut words = [None; 16];
        let (n, body) = match ins.enc {
            Enc::Flat => {
                let [Opnd::Vgpr(addr), _, scalar] = ins.src.as_slice() else {
                    return None;
                };
                let count = match ins.op {
                    12 => 1,
                    13 => 2,
                    14 => 4,
                    15 => 3,
                    _ => return None,
                };
                if ins.aux.mask != 2 {
                    return None;
                }
                let offset = u32::try_from(ins.aux.imm).ok()?;
                let (lo, hi) = if matches!(scalar, Opnd::Null) {
                    (
                        regs.get(&ins.src[0]),
                        regs.words[128 + usize::from(*addr) + 1],
                    )
                } else {
                    (
                        regs.get(&ins.src[0])
                            .zip(regs.get(scalar))
                            .map(|(v, s)| v.wrapping_add(s)),
                        regs.words[index(scalar)? + 1],
                    )
                };
                if usize::from(dw) + count > 128 {
                    return None;
                }
                let mut b = String::from("{ ");
                if matches!(scalar, Opnd::Null) && regs.code_address[128 + usize::from(*addr)] {
                    // RDNA reads the whole address before the load updates vdata,
                    // including when the destination overlaps the address pair.
                    write!(
                        b,
                        "uint2 at = uint2(v[{addr}], v[{}]); ",
                        usize::from(*addr) + 1
                    )
                    .ok()?;
                    for i in 0..count {
                        write!(
                            b,
                            "v[{}] = tbMaterialCode(at.x + {}u, at.y); ",
                            usize::from(dw) + i,
                            offset + i as u32 * 4
                        )
                        .ok()?;
                    }
                } else if matches!(hi, Some(1 | 2)) {
                    let resource = if hi == Some(1) {
                        "tbIndices"
                    } else {
                        "tbVertices"
                    };
                    match scalar {
                        Opnd::Null => write!(b, "uint at = v[{addr}]; ").ok()?,
                        Opnd::Sgpr(base) => write!(b, "uint at = v[{addr}] + s[{base}]; ").ok()?,
                        _ => return None,
                    }
                    for lane in 0..count {
                        write!(
                            b,
                            "v[{}] = {resource}.Load(at + {}u); ",
                            usize::from(dw) + lane,
                            offset.checked_add(4 * lane as u32)?
                        )
                        .ok()?;
                    }
                } else {
                    if hi.is_some_and(|word| word != 0) {
                        return None;
                    }
                    let at = lo?.checked_add(offset)?;
                    for (i, out) in words[..count].iter_mut().enumerate() {
                        let value = match local_word(at.checked_add(4 * i as u32)?)? {
                            Word::Immediate(x) => {
                                *out = Some(x);
                                format!("{x}u")
                            }
                            Word::Value(expr) => expr.to_owned(),
                            Word::InverseTransform(row, col) => {
                                format!("asuint(WorldToObject3x4()[{row}][{col}])")
                            }
                        };
                        write!(b, "v[{}] = {value}; ", usize::from(dw) + i).ok()?;
                    }
                }
                b.push('}');
                (count, Some(b))
            }
            Enc::Smem if ins.op <= 3 && regs.code_address[index(ins.src.first()?)?] => {
                let [Opnd::Sgpr(base)] = ins.src.as_slice() else {
                    return None;
                };
                let offset = u32::try_from(ins.aux.imm).ok()?;
                if *base >= 127 || ins.raw.get(1)? >> 25 != 125 {
                    return None;
                }
                let count = 1usize << ins.op;
                if usize::from(dw) + count > 128 {
                    return None;
                }
                let mut body = format!("{{ uint2 at = uint2(s[{base}], s[{}]); ", base + 1);
                for lane in 0..count {
                    write!(
                        body,
                        "s[{}] = tbMaterialCode(at.x + {}u, at.y); ",
                        usize::from(dw) + lane,
                        offset.checked_add(4 * lane as u32)?
                    )
                    .ok()?;
                }
                body.push('}');
                (count, Some(body))
            }
            Enc::Smem => {
                if !matches!(ins.op, 2 | 3) || ins.aux.imm != 0 || (ins.raw.get(1)? >> 25) != 125 {
                    return None;
                }
                words[0] = regs.get(ins.src.first()?);
                words[0]?;
                // The following use consumes a descriptor through the table.
                // None of the native descriptor words are read as shader data.
                (1 << ins.op, Some(String::new()))
            }
            Enc::Mubuf if !super::super::continuation::scratch(ins) => (
                if ins.op < 4 {
                    (ins.op + 1) as usize
                } else {
                    match ins.op {
                        12 => 1,
                        13 => 2,
                        14 => 4,
                        15 => 3,
                        _ => return None,
                    }
                },
                Some(self.buffer(ins, regs, dw)?),
            ),
            Enc::Mimg => (
                ins.aux.mask.count_ones() as usize,
                Some(self.image(ins, regs, dw)?),
            ),
            _ => {
                if ins.mnemonic.contains("store")
                    || ins.mnemonic == "v_writelane_b32"
                    || ins.mnemonic == "v_nop"
                {
                    return Some(());
                }
                words[..2].copy_from_slice(&regs.alu(ins));
                let n = if ins.enc == Enc::Mubuf {
                    match ins.op {
                        12 | 13 | 14 | 15 => [1, 2, 4, 3][ins.op as usize - 12],
                        _ => 1,
                    }
                } else if ins.mnemonic.ends_with("_b64") {
                    2
                } else {
                    1
                };
                (n, None)
            }
        };
        if let Some(body) = body {
            self.emitted.insert(ins.offset, body);
        }
        if let Some(k) = dst {
            regs.words.get_mut(k..k + n)?.copy_from_slice(&words[..n]);
            regs.code_address.get_mut(k..k + n)?.fill(false);
            regs.code_address[k] = is_table_pointer && whole_sdwa_words(ins);
        }
        Some(())
    }

    fn finish(self, export: &str) -> Bindings {
        let mut declarations = local_record(export);
        declarations.push_str(GEOMETRY);
        if self.views.values().any(|view| *view == View::Cube) {
            declarations.push_str(super::lumen::CUBE_DIRECTION);
        }
        for ((_, slot), view) in self.views {
            match view {
                View::Constant=>writeln!(declarations,"cbuffer RecordCb{slot} : register(b{slot},space0) {{ uint4 tbCb{slot}[4096]; }};"),
                View::Sampler=>writeln!(declarations,"SamplerState tbSampler{slot} : register(s{slot},space0);"),
                kind=>{
                    let ty=match kind{View::Structured=>"StructuredBuffer<uint4>",View::Typed=>"Buffer<float4>",View::Page=>"Texture2D<uint4>",View::Texture=>"Texture2D<float4>",View::Volume=>"Texture3D<float4>",View::Cube=>"TextureCube<float4>",View::Array=>"Texture2DArray<float4>",_=>unreachable!()};
                    writeln!(declarations,"{ty} tbSrv{slot} : register(t{slot},space0);")
                }
            }.expect("writing a String");
        }
        Bindings {
            declarations,
            instructions: self.emitted,
        }
    }
}

pub(super) fn analyze(
    state: &State<'_>,
    span: &Range<usize>,
    candidate: bool,
) -> Result<Bindings, String> {
    let mut regs = Values::new();
    if candidate {
        for reg in [2, 3, 5] {
            regs.words[128 + reg] = None;
        }
        regs.words[128 + 26] = Some(8);
        regs.words[128 + 27] = Some(8);
        regs.words[128 + 29] = Some(0);
    }
    let constants = super::super::continuation::leaf_constants(state);
    let mut layout = Layout::default();
    if let Some(range) = &constants {
        let base = super::super::continuation::CODE_BASE;
        layout.constant_addresses = base + range.start as u32..base + range.end as u32;
    }

    let mut saved_exec = BTreeMap::new();
    let isa = state
        .program
        .get(span.clone())
        .ok_or("invalid ray material substate range")?;
    let ops: Vec<_> = instructions(isa)
        .map(|mut op| {
            op.offset += span.start;
            op
        })
        .collect();
    // A compiler-outlined arm following an unconditional branch is not
    // reached by falling through the preceding instructions. Recover its
    // exact incoming values when it has one forward control-flow edge.
    let mut incoming = BTreeMap::new();
    for op in &ops {
        if let Some(dest) = super::super::continuation::target(op) {
            incoming
                .entry(dest)
                .and_modify(|source| *source = None)
                .or_insert(Some(op.offset));
        }
    }
    let mut arms = BTreeMap::new();
    let mut falls_through = true;
    for op in &ops {
        if !falls_through {
            if let Some(source) = incoming.get(&op.offset) {
                if let Some(source) = source.filter(|&source| source < op.offset) {
                    arms.insert(source, op.offset);
                }
                falls_through = true;
            }
        }
        // Alignment nops between an unconditional transfer and the actual
        // forward target do not introduce a predecessor.
        if op.mnemonic != "s_nop" {
            falls_through = !matches!(op.mnemonic.as_str(), "s_branch" | "s_setpc_b64");
        }
    }
    for (i, op) in ops
        .iter()
        .enumerate()
        .filter(|(_, op)| op.mnemonic == "v_movreld_b32")
    {
        if let Some(range) = relative::bounded_write(&ops, i) {
            layout.relative_writes.insert(op.offset, range);
        }
    }
    let mut saved_arms = BTreeMap::new();
    for ins in ops {
        if ins.mnemonic == "image_load"
            && !image::packed_load(&ins, &state.program[ins.offset + ins.dwords * 4..span.end])
        {
            return Err(format!(
                "unresolved material image_load element type at {:#x}",
                ins.offset
            ));
        }
        if let Some((r, spills, masks)) = saved_arms.remove(&ins.offset) {
            regs = r;
            layout.spills = spills;
            saved_exec = masks;
        }
        if let Some(&dest) = arms.get(&ins.offset) {
            saved_arms.insert(
                dest,
                (regs.clone(), layout.spills.clone(), saved_exec.clone()),
            );
        }
        // The ELSE side of an EXEC-masked block sees the original VGPRs
        // in those lanes. The THEN path may have reused them for load data.
        if ins.mnemonic == "s_and_saveexec_b32" {
            if let [Opnd::Sgpr(saved)] = ins.dst.as_slice() {
                saved_exec.insert(*saved, std::rc::Rc::new(regs.clone()));
            }
        } else if ins.mnemonic == "s_andn2_b32" && ins.dst == [Opnd::ExecLo] {
            if let [Opnd::Sgpr(saved), Opnd::ExecLo] = ins.src.as_slice() {
                if let Some(entry) = saved_exec.get(saved) {
                    regs.words[128..].copy_from_slice(&entry.words[128..]);
                    regs.code_address[128..].copy_from_slice(&entry.code_address[128..]);
                }
            }
        }
        layout.instruction(&ins, &mut regs).ok_or_else(|| {
            format!(
                "unresolved material resource {} at {:#x}",
                ins.mnemonic, ins.offset
            )
        })?;
    }
    let base = layout.constant_addresses.start;
    let mut bindings = layout.finish(&state.name);
    if let Some(range) = constants {
        let words = state.program[range]
            .chunks_exact(4)
            .map(|bytes| format!("0x{:08x}u", u32::from_le_bytes(bytes.try_into().unwrap())))
            .collect::<Vec<_>>();
        writeln!(bindings.declarations, "uint tbMaterialCode(uint lo, uint hi) {{ static const uint words[{}] = {{ {} }}; uint offset = lo - {base}u; return hi == 0u && (offset & 3u) == 0u && offset / 4u < {}u ? words[offset / 4u] : 0u; }}", words.len(), words.join(","), words.len()).unwrap();
    }
    Ok(bindings)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(words: &[u32]) -> Insn {
        let code: Vec<_> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        let instruction = instructions(&code).next().unwrap();
        instruction
    }

    #[test]
    fn mul24_folds_signed_overflow_and_preserves_adjacent_bytes() {
        let mut regs = Values::new();
        regs.words[129] = Some(0xab80_0000); // -2^23, or +2^23 unsigned
        regs.words[133] = Some(257);
        regs.words[132] = Some(0xaabb_ccdd);
        for (op, ext, expected) in [
            (0xd509_8004, 0x0002_0b01, 0x8000_0000), // signed saturation
            (0xd50b_8004, 0x0002_0b01, 0x8080_0000), // unsigned fits
            (0x1408_0af9, 0x0606_1101, 0xaabb_ffdd), // signed high word
            (0x1808_0af9, 0x0606_1101, 0xaabb_00dd), // unsigned high word
        ] {
            assert_eq!(regs.alu(&decode(&[op, ext]))[0], Some(expected));
        }
        // 65536 squared is the first value above the unsigned dword range.
        regs.words[129] = Some(65536);
        regs.words[133] = Some(65536);
        assert_eq!(regs.alu(&decode(&[0xd50b_8004, 0x0002_0b01]))[0], Some(u32::MAX));
        assert_eq!(regs.alu(&decode(&[0xd50b_0004, 0x0002_0b01]))[0], Some(0));
    }

    #[test]
    fn sdwa_minmax_folds_using_the_instruction_signedness() {
        let mut regs = Values::new();
        regs.words[129] = Some(0x8000_00fe);
        regs.words[133] = Some(0x7fff_ff01);
        regs.words[132] = Some(0xaabb_ccdd);
        for (opcode, expected) in [
            (0x2208_0af9, 0xaabb_fedd), // signed minimum
            (0x2408_0af9, 0xaabb_01dd), // signed maximum
            (0x2608_0af9, 0xaabb_01dd), // unsigned minimum
            (0x2808_0af9, 0xaabb_fedd), // unsigned maximum
        ] {
            assert_eq!(regs.alu(&decode(&[opcode, 0x0606_1101]))[0], Some(expected));
        }
    }

    #[test]
    fn sdwa_right_shift_preserves_the_material_descriptor_bank() {
        let mut regs = Values::new();
        let mut layout = Layout::default();
        regs.words[129] = Some(33);
        regs.words[133] = Some(0x8000_0040);
        regs.words[132] = Some(SRV);
        // v_lshrrev_b32 v4,v1,v5: replace only BYTE_0 of the SRV address.
        let shift = decode(&[0x2c08_0af9, 0x0606_1001]);
        layout.instruction(&shift, &mut regs).unwrap();
        assert_eq!(regs.words[132], Some(SRV + 32));
        let address = regs.words[132].unwrap();
        assert_eq!(layout.resource(address, SRV, View::Structured).as_deref(), Some("tbSrv1"));
        regs.words[132] = None;
        layout.instruction(&shift, &mut regs).unwrap();
        assert_eq!(regs.words[132], None);
    }

    #[test]
    fn sdwa_add_does_not_carry_pointer_provenance_through_a_partial_write() {
        let mut layout = Layout::default();
        let mut regs = Values::new();
        regs.code_address[129] = true;
        let add = |ext| decode(&[0x4a08_0af9, ext]);
        layout.instruction(&add(0x0606_0601), &mut regs).unwrap();
        assert!(regs.code_address[132]);
        layout.instruction(&add(0x0606_1101), &mut regs).unwrap();
        assert!(!regs.code_address[132]);
    }

    #[test]
    fn flat_three_word_read_preserves_adjacent_live_descriptor() {
        let mut regs = Values::new();
        let mut layout = Layout::default();
        // global_load_dwordx3 v[51:53], v[51:52], off (triangle vertices).
        let load = decode(&[3694952448, 863830067]);
        regs.words[128 + 51] = None;
        regs.words[128 + 52] = Some(2);
        regs.words[128 + 54] = Some(SRV + 32);
        layout.instruction(&load, &mut regs).unwrap();
        assert_eq!(regs.words[128 + 54], Some(SRV + 32));
        assert!(layout.emitted[&0].contains("tbVertices.Load(at + 8u)"));
    }

    #[test]
    fn recovers_traversal_address_after_private_vgpr_spill() {
        let mut regs = Values::new();
        let mut layout = Layout::default();
        // Material shaders spill the traversal address in v16 while shading
        // and reload it for a second PrimitiveIndex lookup.
        let store = decode(&[3765436464, 83890176]);
        let load = decode(&[3761242160, 83890176]);
        layout.instruction(&store, &mut regs).unwrap();
        regs.words[128 + 16] = None;
        layout.instruction(&load, &mut regs).unwrap();
        assert_eq!(regs.words[128 + 16], Some(BVH_BASE >> 8));
    }

    #[test]
    fn separate_slot_types_share_numbers_in_different_tables() {
        let mut layout = Layout::default();
        assert_eq!(
            layout.resource(SRV + 64, SRV, View::Structured).as_deref(),
            Some("tbSrv2")
        );
        assert_eq!(
            layout.resource(CBV + 64, CBV, View::Constant).as_deref(),
            Some("tbCb2")
        );
        assert!(layout.resource(SRV + 64, SRV, View::Texture).is_none());
        assert!(
            layout
                .resource(SAMPLER + 3, SAMPLER, View::Sampler)
                .is_none()
        );
    }
}
