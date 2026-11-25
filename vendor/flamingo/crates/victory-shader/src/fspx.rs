//! `FSPX`: the console's precompiled compute programs, as AMD's FidelityFX
//! upscaler ships them. The container has no `DXIL` part; its `ACCC` part
//! holds an image (little-endian words, offsets from the image start):
//!
//! ```text
//! +0x00 "FSPX", u32 version (1), u32 section count, u32 image size
//! +0x10 sections of 16 B: u32 kind, u32 offset, u32 size, u32 0
//! kind 0  compute config, 7 words: two constants (0x4005cf00, 0x6c090102),
//!         0, u16 x u16 y thread-group size, u16 z u16 flags (bit 4 wave32,
//!         else wave64), COMPUTE_PGM_RSRC1, COMPUTE_PGM_RSRC2
//! kind 1  20 B: a 32-bit hash, 3, zeros (not read)
//! kind 6  GFX10 code, uncompressed
//! kind 7  root map: u32 version (0x403), u32 size after this 20-byte
//!         header, u32 code dwords, u32 0, u32 0; u32 0x440 | n << 15 (n twice the
//!         binding count, less one for some: not read);
//!         then entries of 8 words, by tag (word 0):
//!   0x41  a binding: u8 resource type (0 SRV, 1 sampler, 2 CBV, 3 UAV) |
//!         u8 root type (1 root descriptor, 3 table, 4 immediate sampler)
//!         << 8; u32 table offset (dwords, a descriptor takes 8); u32 base
//!         register; u32 view dimension (D3D12: 1 buffer, 4 2D texture);
//!         u8 descriptor dwords | u8 first user-data dword << 8 | u8 root
//!         slot << 16; u32 -1; u32 0
//!   0x42  a declaration: u32 0; u8 1 | u8 class (0 SRV, 1 UAV) << 8 |
//!         u16 shape << 16; u32 register; zeros (not read)
//!   0x43  4 words of hash, zeros (not read)
//! ```
//!
//! Each user-data dword is a user SGPR, as in a compute program of a `DXIL`
//! part ([`crate::native`]): a table in one, a root descriptor (64-bit
//! address) in two. The bindings are the program's used resources; the root
//! layout is one entry per root slot they name.

use crate::ShaderError::{self, Malformed};
use crate::dxbc::u32_at;
use crate::native::{
    ComputeIo, HwStage, NativeProgram, ResourceType, RootParam, RootType, StageIo, UsedResource,
};

pub const MAGIC: &[u8; 4] = b"FSPX";
const VERSION: u32 = 1;
const MAX_SECTIONS: u32 = 16;
const KIND_CONFIG: u32 = 0;
const KIND_CODE: u32 = 6;
const KIND_MAP: u32 = 7;
const CONFIG_WORDS: usize = 7;
const FLAG_WAVE32: u32 = 1 << 4;
const MAP_VERSION: u32 = 0x403;
const MAP_HEADER: usize = 24;
const ENTRY: usize = 32;
const TAG_BINDING: u32 = 0x41;
const TAG_DECLARATION: u32 = 0x42;
const TAG_HASH: u32 = 0x43;

/// The program of an `FSPX` image (an `ACCC` part's data).
pub fn program(image: &[u8]) -> Result<NativeProgram, ShaderError> {
    if image.get(0..4) != Some(MAGIC) {
        return Err(Malformed("ACCC part lacks FSPX magic"));
    }
    if u32_at(image, 4)? != VERSION {
        return Err(Malformed("unknown FSPX version"));
    }
    let count = u32_at(image, 8)?;
    if count > MAX_SECTIONS {
        return Err(Malformed("implausible FSPX section count"));
    }
    let image = image
        .get(..u32_at(image, 12)? as usize)
        .ok_or(Malformed("FSPX image size exceeds its part"))?;
    let (mut config, mut code, mut map) = (None, None, None);
    for i in 0..count as usize {
        let at = 16 + 16 * i;
        let (kind, offset, size) = (
            u32_at(image, at)?,
            u32_at(image, at + 4)?,
            u32_at(image, at + 8)?,
        );
        let section = (offset as usize)
            .checked_add(size as usize)
            .and_then(|end| image.get(offset as usize..end))
            .ok_or(Malformed("FSPX section out of bounds"))?;
        let slot = match kind {
            KIND_CONFIG => &mut config,
            KIND_CODE => &mut code,
            KIND_MAP => &mut map,
            _ => continue,
        };
        if slot.replace(section).is_some() {
            return Err(Malformed("FSPX section repeated"));
        }
    }
    let config = config.ok_or(Malformed("FSPX image lacks its compute config"))?;
    let code = code.ok_or(Malformed("FSPX image lacks code"))?;
    let map = map.ok_or(Malformed("FSPX image lacks its root map"))?;
    if config.len() < 4 * CONFIG_WORDS {
        return Err(Malformed("FSPX compute config truncated"));
    }
    if code.is_empty() || !code.len().is_multiple_of(4) {
        return Err(Malformed("FSPX code is not whole dwords"));
    }
    let word = |i: usize| u32_at(config, 4 * i).expect("in bounds");
    let (size_xy, size_z, rsrc1, rsrc2) = (word(3), word(4), word(5), word(6));
    let wave64 = size_z >> 16 & FLAG_WAVE32 == 0;
    let threads = [size_xy & 0xFFFF, size_xy >> 16, size_z & 0xFFFF];
    if threads.contains(&0) || threads.iter().product::<u32>() > 1024 {
        return Err(Malformed("implausible FSPX thread-group size"));
    }
    // COMPUTE_PGM_RSRC1 bits 0-5: VGPR granules less one, of 4 VGPRs in
    // wave64 and 8 in wave32.
    let vgprs = ((rsrc1 & 0x3F) + 1) * if wave64 { 4 } else { 8 };
    let (layout, used) = root_map(map, code.len())?;
    Ok(NativeProgram {
        stage: HwStage::Cs,
        wave64,
        vgprs: vgprs.min(256) as u16,
        layout,
        used,
        io: StageIo::Cs(ComputeIo { rsrc2, threads }),
        code: code.to_vec(),
    })
}

/// The root layout and used resources of a kind 7 section.
fn root_map(
    map: &[u8],
    code_len: usize,
) -> Result<(Vec<RootParam>, Vec<UsedResource>), ShaderError> {
    if u32_at(map, 0)? != MAP_VERSION {
        return Err(Malformed("unknown FSPX root map version"));
    }
    if u32_at(map, 4)? as usize + 20 != map.len() {
        return Err(Malformed("FSPX root map size disagrees"));
    }
    if u32_at(map, 8)? as usize * 4 != code_len {
        return Err(Malformed("FSPX root map names another code size"));
    }
    let mut used = Vec::new();
    for e in map[MAP_HEADER.min(map.len())..].chunks(ENTRY) {
        let w = |i: usize| u32_at(e, 4 * i);
        match w(0)? {
            TAG_BINDING => {
                let (types, place) = (w(1)?, w(5)?);
                used.push(UsedResource {
                    kind: match types & 0xFF {
                        0 => ResourceType::Srv,
                        1 => ResourceType::Sampler,
                        2 => ResourceType::Cbv,
                        3 => ResourceType::Uav,
                        n => ResourceType::Other(n as u8),
                    },
                    root: match types >> 8 & 0xFF {
                        1 => RootType::Descriptor,
                        3 => RootType::Table,
                        4 => RootType::ImmediateSampler,
                        n => RootType::Other(n),
                    },
                    table_offset_dwords: w(2)?,
                    lower_bound: w(3)?,
                    // shortcut: no structure stride is stored; FSR reads its
                    // buffers as typed or raw ones.
                    stride: 0,
                    dwords: place as u8,
                    user_data: (place >> 8) as u8,
                    slot: (place >> 16) as u8,
                });
            }
            TAG_DECLARATION | TAG_HASH => {}
            _ => return Err(Malformed("unknown FSPX root map entry")),
        }
    }
    // One layout entry per root slot: a table's user SGPR, a root
    // descriptor's two; none for an immediate sampler.
    let mut layout: Vec<RootParam> = Vec::new();
    for u in &used {
        if layout.iter().any(|p| p.slot == u.slot && p.root == u.root) {
            continue;
        }
        let sgprs = match u.root {
            RootType::Table => 1,
            RootType::Descriptor => 2,
            // Fixed in the code (it builds the sampler's words itself).
            _ => 0,
        };
        layout.push(RootParam {
            root: u.root,
            table_offset_dwords: 0,
            sgprs,
            base_register: if u.root == RootType::Table {
                0
            } else {
                u.lower_bound
            },
            user_data: u.user_data,
            slot: u.slot,
        });
    }
    layout.sort_by_key(|p| p.user_data);
    Ok((layout, used))
}

#[cfg(test)]
mod tests {
    use super::*;

    const S_ENDPGM: u32 = 0xBF81_0000;

    fn words(w: &[u32]) -> Vec<u8> {
        w.iter().flat_map(|x| x.to_le_bytes()).collect()
    }

    /// An image as the upscaler's 32-lane passes have it: a UAV table in
    /// user SGPR 4 (u11 at table dword 88), a root CBV (b0, slot 2) in
    /// SGPRs 6-7, a code-built immediate sampler.
    fn image() -> Vec<u8> {
        let config = words(&[
            0x4005_cf00,
            0x6c09_0102,
            0,
            0x1_0020,
            0x10_0001,
            0x402c_0341,
            0x9c,
        ]);
        let code = words(&[S_ENDPGM]);
        let mut entries = vec![
            0x41,
            0x303,
            88,
            11,
            1,
            0x404,
            u32::MAX,
            0, // u11, slot 0, user data 4
            0x41,
            0x102,
            0,
            0,
            0,
            0x2_0602,
            u32::MAX,
            0, // b0, slot 2, user data 6
            0x41,
            0x401,
            0,
            0,
            0,
            0x4,
            u32::MAX,
            0, // s0, immediate
            0x42,
            0,
            0x2_0101,
            11,
            0,
            0,
            u32::MAX,
            0, // declaration
        ];
        entries.extend([0x43, 1, 2, 3, 4, 0, 0, 0]);
        let mut map = vec![MAP_VERSION, 0, 1, 0, 0, 3 << 16 | 0x440];
        map.extend(entries);
        map[1] = (4 * map.len() - 20) as u32;
        let map = words(&map);
        let sections = [(KIND_CONFIG, config), (KIND_CODE, code), (KIND_MAP, map)];
        let mut at = 16 + 16 * sections.len();
        let mut dir = Vec::new();
        let mut body: Vec<u8> = Vec::new();
        for (kind, data) in &sections {
            dir.extend(words(&[*kind, at as u32, data.len() as u32, 0]));
            body.extend(data);
            at += data.len();
        }
        let mut out = MAGIC.to_vec();
        out.extend(words(&[VERSION, sections.len() as u32, at as u32]));
        out.extend(dir);
        out.extend(body);
        out
    }

    #[test]
    fn reads_config_code_and_root_map() {
        let p = program(&image()).unwrap();
        assert_eq!(p.stage, HwStage::Cs);
        assert!(!p.wave64);
        assert_eq!(p.vgprs, 16);
        assert_eq!(p.code, S_ENDPGM.to_le_bytes());
        let StageIo::Cs(cs) = p.io else { panic!() };
        assert_eq!(cs.threads, [32, 1, 1]);
        assert_eq!(cs.user_sgprs(), 14);
        assert_eq!(cs.system_sgprs(), ([Some(14), None, None], None));
        assert_eq!(
            p.used[0],
            UsedResource {
                kind: ResourceType::Uav,
                root: RootType::Table,
                table_offset_dwords: 88,
                lower_bound: 11,
                stride: 0,
                dwords: 4,
                user_data: 4,
                slot: 0,
            }
        );
        assert_eq!(
            (p.used[1].kind, p.used[1].root),
            (ResourceType::Cbv, RootType::Descriptor)
        );
        assert_eq!(p.used[2].kind, ResourceType::Sampler);
        let layout: Vec<_> = p
            .layout
            .iter()
            .map(|l| (l.root, l.user_data, l.sgprs, l.slot))
            .collect();
        assert_eq!(
            layout,
            [
                (RootType::ImmediateSampler, 0, 0, 0),
                (RootType::Table, 4, 1, 0),
                (RootType::Descriptor, 6, 2, 2),
            ]
        );
        // The container's ACCC part reaches it through `native_programs`.
        assert_eq!(crate::native_programs(&image()).unwrap(), [p]);
    }

    #[test]
    fn rejects_corrupt_images() {
        let good = image();
        let mut bad = good.clone();
        bad[4] = 2; // version
        assert!(program(&bad).is_err());
        let mut bad = good.clone();
        bad[12] = 0xFF; // image size
        assert!(program(&bad).is_err());
        assert!(program(&good[..good.len() - 4]).is_err());
        let mut bad = good.clone();
        let tag = good.len() - 32 * 5; // the first root map entry's tag
        bad[tag] = 0x44;
        assert!(program(&bad).is_err());
    }
}
