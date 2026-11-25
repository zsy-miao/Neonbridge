//! Independent lowering for empty dispatches and stores of constants to a
//! single structured UAV. Native load/store fields follow AMD's GFX10 ISA.
//! The FSSM tables below are read from Series X compute programs.

pub(crate) mod page_list;
mod compact_views;
mod visualize_tiles;
mod mesh_sdf_offsets;
mod material_sort;
mod compact_visualize_rays;
mod compact_hwrt_rays;
mod distance_field_requests;
mod voxel_indirect_setup;
mod lumen_update_tiles;

use std::fmt::Write;

use crate::rdna2::{instructions, Enc, Opnd};
use crate::rootsig::RootParamKind;
use crate::{ShaderExtract, Stage};

fn word(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(*bytes.get(at..)?.first_chunk()?))
}

fn section(bytes: &[u8], directory: &[u8], at: usize) -> Option<usize> {
    let offset = u64::from_le_bytes(*directory.get(at..)?.first_chunk()?);
    let offset = usize::try_from(offset).ok()?;
    (offset < bytes.len()).then_some(offset)
}

// This layout has four user-data tables at s4..s7. The last table contains
// UAV descriptors. The resource record supplies the byte stride which is not
// present in the desktop pipeline-validation binding. Cross-check the PSV
// binding, RTS0 descriptor range and both FSSM tables before using s7.
fn structured_stride(shader: &ShaderExtract) -> Option<u32> {
    let [resource] = shader.resources.as_slice() else {
        return None;
    };
    if (
        resource.res_type,
        resource.space,
        resource.lower,
        resource.upper,
    ) != (8, 0, 0, 0)
    {
        return None;
    }
    let root = shader.root_sig.as_ref()?;
    let uavs = root.params.get(3)?;
    let RootParamKind::Table(ranges) = &uavs.kind else {
        return None;
    };
    let range = ranges.first()?;
    if uavs.visibility != 0
        || range.range_type != 1
        || range.space != 0
        || range.base_register != 0
        || range.num == 0
        || !matches!(range.offset_in_table, 0 | u32::MAX)
    {
        return None;
    }
    let size = u64::from_le_bytes(*shader.fssm.get(8..)?.first_chunk()?);
    let image = shader.fssm.get(16..)?.get(..usize::try_from(size).ok()?)?;
    single_uav_stride(image)
}

fn series_resource_bindings(image: &[u8]) -> Option<&[u8]> {
    if image.get(..8)? != 6u64.to_le_bytes() {
        return None;
    }
    let descriptor = image.get(section(image, image, 0x60)?..)?;
    if word(descriptor, 0x38)? != 4 {
        return None;
    }
    let table = image.get(section(image, descriptor, 0x28)?..)?.get(..80)?;
    for (index, record) in table.chunks_exact(20).enumerate() {
        let expected = [3, 0, 1, 0, ((index as u32) << 8) | (4 + index as u32)];
        if !record
            .chunks_exact(4)
            .zip(expected)
            .all(|(v, n)| v == n.to_le_bytes())
        {
            return None;
        }
    }
    let count = usize::try_from(word(descriptor, 0x3c)?).ok()?;
    image
        .get(section(image, descriptor, 0x30)?..)?
        .get(..count.checked_mul(20)?)
}

fn single_uav_stride(image: &[u8]) -> Option<u32> {
    let binding = series_resource_bindings(image)?;
    if binding.len() != 20 {
        return None;
    }
    let stride = word(binding, 12)?;
    if word(binding, 0)? != 0x303
        || word(binding, 4)? != 0
        || word(binding, 8)? != 0
        || word(binding, 16)? != 0x30704
        || !(4..=2048).contains(&stride)
        || stride % 4 != 0
    {
        return None;
    }
    Some(stride)
}

fn group_size(shader: &ShaderExtract) -> Option<[u32; 3]> {
    let dims @ [x, y, z] = shader.psv_threads?;
    (x != 0 && y != 0 && (1..=64).contains(&z) && x.checked_mul(y)?.checked_mul(z)? <= 1024)
        .then_some(dims)
}

fn no_effect(ins: &crate::rdna2::Insn) -> bool {
    matches!((ins.enc, ins.op), (Enc::Sopp, 0 | 32) | (Enc::Vop3, 0x180))
}

#[derive(Clone, Copy, PartialEq)]
enum Descriptor {
    Unknown,
    Low,
    Zero,
    Word(u8),
}

// Emit in native instruction order. Only singleton groups and constant byte
// addresses qualify. Each native dword writes a field of element zero, so
// adjacent fields stay intact even in records wider than a uint.
fn store_body(isa: &[u8], stride: u32) -> Option<String> {
    let mut scalar = [Descriptor::Unknown; 102];
    scalar[7] = Descriptor::Low;
    let mut vector = [None; 256];
    let mut stores = String::new();
    let mut waited = false;
    for ins in instructions(isa) {
        if !ins.is_complete() {
            return None;
        }
        if no_effect(&ins) {
            continue;
        }
        match (ins.enc, ins.op) {
            (Enc::Sopp, 1) if ins.raw == [0xbf810000] && ins.offset + 4 == isa.len() => {
                return (!stores.is_empty()).then_some(stores);
            }
            (Enc::Sop1, 3) => {
                let ([Opnd::Sgpr(dst)], [src]) = (&ins.dst[..], &ins.src[..]) else {
                    return None;
                };
                let value = match src {
                    Opnd::Sgpr(reg) => *scalar.get(*reg as usize)?,
                    Opnd::ConstInt(0) => Descriptor::Zero,
                    _ => return None,
                };
                *scalar.get_mut(*dst as usize)? = value;
            }
            (Enc::Vop1, 1) if ins.aux.sdwa.is_none() => {
                let ([Opnd::Vgpr(dst)], [source]) = (&ins.dst[..], &ins.src[..]) else {
                    return None;
                };
                vector[*dst as usize] = Some(match source {
                    Opnd::ConstInt(n) => *n as u32,
                    Opnd::ConstF32(f) => f.to_bits(),
                    Opnd::Literal(bits) => *bits,
                    Opnd::Vgpr(reg) => vector[*reg as usize]?,
                    _ => return None,
                });
            }
            (Enc::Smem, 2) if ins.raw[0] & !0x1fff == 0xf4080000 && ins.raw[1] == 0xfa000000 => {
                let ([Opnd::Sgpr(dst)], [Opnd::Sgpr(base)]) = (&ins.dst[..], &ins.src[..]) else {
                    return None;
                };
                let addr = *base as usize;
                if scalar.get(addr..addr + 2)? != [Descriptor::Low, Descriptor::Zero] {
                    return None;
                }
                for (i, slot) in scalar
                    .get_mut(*dst as usize..)?
                    .get_mut(..4)?
                    .iter_mut()
                    .enumerate()
                {
                    *slot = Descriptor::Word(i as u8);
                }
                waited = false;
            }
            (Enc::Sopp, 12) if ins.raw == [0xbf8cc07f] => waited = true, // lgkmcnt(0)
            (Enc::Mubuf, 28..=31)
                if waited
                    && ins.raw[0] & !0xfff == (0xe0000000 | (ins.op << 18))
                    && ins.raw[1] & 0xffe0_0000 == 0x8000_0000 =>
            {
                let ([Opnd::Vgpr(data)], [_, Opnd::Sgpr(base), Opnd::ConstInt(0)]) =
                    (&ins.dst[..], &ins.src[..])
                else {
                    return None;
                };
                let desc = scalar.get(*base as usize..)?.get(..4)?;
                if !desc
                    .iter()
                    .enumerate()
                    .all(|(i, &v)| v == Descriptor::Word(i as u8))
                {
                    return None;
                }
                let bytes = ins.raw[0] & 0xfff;
                let width = [1, 2, 4, 3][(ins.op - 28) as usize];
                if bytes % 4 != 0 || bytes + 4 * width > stride {
                    return None;
                }
                for i in 0..width {
                    let value = vector.get(*data as usize + i as usize).copied().flatten()?;
                    writeln!(
                        stores,
                        "    output[0].words[{}] = {value:#010x}u;",
                        bytes / 4 + i
                    )
                    .ok()?;
                }
            }
            _ => return None,
        }
    }
    None
}

pub(crate) fn card_direct_shadow(shader: &ShaderExtract) -> bool {
    shader.stage == Stage::Compute
        && shader.entry.as_deref() == Some("LumenSceneDirectLightingHardwareRayTracingCS")
        && shader.psv_threads == Some([8, 4, 1])
        && shader.isa.len() == 5048
        && shader.resources.iter().all(|r| r.space == 0 && r.lower == r.upper)
        && shader.resources.iter().map(|r| (r.res_type, r.lower)).eq([
            (2, 0), (2, 1), (2, 2), (1, 0), (5, 0), (5, 1), (3, 2), (3, 3),
            (5, 4), (4, 5), (5, 6), (8, 0),
        ])
        && shader.isa.iter().fold(0xcbf29ce484222325u64, |hash, &byte|
            (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)) == 0x97c181838546ca6e
}

pub(super) fn hlsl(shader: &ShaderExtract) -> Option<String> {
    if shader.stage != Stage::Compute
        || !shader.has_psv0
        || !shader.has_input_sig
        || !shader.has_output_sig
        || !shader.input_sig.is_empty()
        || !shader.output_sig.is_empty()
        || !shader.primitive_sig.is_empty()
        || shader.isa.len() % 4 != 0
    {
        return None;
    }
    let dims @ [x, y, z] = group_size(shader)?;
    if shader.resources.is_empty() {
        let mut ops = instructions(&shader.isa);
        let end = ops.find(|op| !op.is_complete() || !no_effect(op))?;
        if end.raw != [0xbf810000] || ops.next().is_some() {
            return None;
        }
        return Some(format!("[numthreads({x}, {y}, {z})]\nvoid main() {{}}\n"));
    }
    if card_direct_shadow(shader) {
        return Some(include_str!("compute_hlsl/card_direct_shadow.hlsl").to_owned());
    }
    if let Some(source) = lumen_update_tiles::translate(shader) {
        return Some(source);
    }
    if let Some(source) = voxel_indirect_setup::translate(shader) {
        return Some(source);
    }
    if let Some(source) = distance_field_requests::translate(shader) {
        return Some(source);
    }
    if let Some(source) = compact_hwrt_rays::translate(shader) {
        return Some(source);
    }
    if let Some(source) = compact_visualize_rays::translate(shader) {
        return Some(source);
    }
    if let Some(source) = material_sort::translate(shader) {
        return Some(source);
    }
    if let Some(source) = mesh_sdf_offsets::translate(shader) {
        return Some(source);
    }
    if let Some(source) = visualize_tiles::translate(shader) {
        return Some(source);
    }
    if let Some(source) = compact_views::translate(shader) {
        return Some(source);
    }
    if let Some(source) = page_list::pack_free_pages(shader) {
        return Some(source);
    }
    if dims != [1, 1, 1] {
        return None;
    }
    let stride = structured_stride(shader)?;
    let body = store_body(&shader.isa, stride)?;
    Some(format!("struct Element {{ uint words[{}]; }};\nRWStructuredBuffer<Element> output : register(u0);\n[numthreads(1, 1, 1)]\nvoid main() {{\n{body}}}\n",stride / 4))
}

#[cfg(test)]
#[path = "tests/compute_hlsl.rs"]
mod tests;
