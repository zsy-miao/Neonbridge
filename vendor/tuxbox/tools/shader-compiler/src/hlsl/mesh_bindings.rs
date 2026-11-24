//! Bind mesh resources whose FSSM layout agrees with the pipeline root.
use crate::{
    meta::{RootParamKind, RootRange},
    ShaderExtract,
};
use std::collections::{BTreeMap, BTreeSet};
use tuxbox_dxbc::compute_table::{records, user_data_records};

#[derive(Clone, Copy)]
pub(super) struct TableBinding {
    pub kind: u32,
    pub register: u32,
    pub space: u32,
    pub stride_words: Option<u32>,
}

pub(super) struct MeshBindings {
    pub tables: BTreeMap<(u8, u32), TableBinding>,
    pub raw: BTreeMap<u8, (u32, u32)>,
}

impl MeshBindings {
    pub(super) fn read(ex: &ShaderExtract) -> Option<Self> {
        let root = ex.root_sig.as_ref()?;
        let used = records(&ex.fssm)?;
        let layout: BTreeSet<_> = user_data_records(&ex.fssm)?.collect();
        if used.len() != ex.resources.len() {
            return None;
        }
        let mut tables = BTreeMap::new();
        let mut raw = BTreeMap::new();
        let mut seen = BTreeSet::new();
        let mut root_slots = BTreeMap::new();
        for [class, offset, register, shape, loc] in used {
            // The location records the root slot, user-data word and descriptor width.
            // Mesh has eight system SGPRs before the user data.
            if loc & 255 != 4 {
                return None;
            }
            let param = root.params.get((loc >> 16) as usize)?;
            let sgpr = ((loc >> 8) as u8).checked_add(8)?;
            if sgpr > 103 || !matches!(param.visibility, 0 | 7) {
                return None;
            }
            if root_slots
                .insert(sgpr, loc >> 16)
                .is_some_and(|slot| slot != loc >> 16)
            {
                return None;
            }
            let (ty, space) = match (class, &param.kind) {
                (0x300 | 0x302, RootParamKind::Table(ranges)) => {
                    let (kind, host_register, space) = table_slot(ranges, offset)?;
                    if register != host_register
                        || !layout.contains(&[3, offset, 1, register, loc >> 8])
                    {
                        return None;
                    }
                    let (ty, stride_words) = match (class, kind, shape) {
                        (0x302, 2, 0) => (2, None),
                        (0x300, 0, 1) => (4, None),
                        (0x300, 0, stride) if (4..=2048).contains(&stride) && stride % 4 == 0 => {
                            (5, Some(stride / 4))
                        }
                        _ => return None,
                    };
                    let binding = TableBinding {
                        kind,
                        register,
                        space,
                        stride_words,
                    };
                    if tables
                        .insert((sgpr, offset.checked_mul(4)?), binding)
                        .is_some()
                    {
                        return None;
                    }
                    (ty, space)
                }
                (
                    0x100,
                    RootParamKind::Descriptor {
                        kind: 3,
                        register: native,
                        space,
                    },
                ) if register == *native
                    && offset == 0
                    && shape == 1
                    && layout.contains(&[1, 0, 2, register, loc >> 8]) =>
                {
                    if raw.insert(sgpr, (register, *space)).is_some() {
                        return None;
                    }
                    (4, *space)
                }
                _ => return None,
            };
            if !seen.insert((ty, space, register))
                || ex
                    .resources
                    .iter()
                    .filter(|p| {
                        (p.res_type, p.space, p.lower, p.upper) == (ty, space, register, register)
                    })
                    .count()
                    != 1
            {
                return None;
            }
        }
        if raw.keys().any(|&sgpr| root_slots.contains_key(&(sgpr + 1))) {
            return None;
        }
        Some(Self { tables, raw })
    }

    pub(super) fn takes_sgpr(&self, sgpr: u8) -> bool {
        self.tables
            .range((sgpr, 0)..=(sgpr, u32::MAX))
            .next()
            .is_some()
            || self.raw.contains_key(&sgpr)
            || sgpr
                .checked_sub(1)
                .is_some_and(|low| self.raw.contains_key(&low))
    }
}

fn table_slot(ranges: &[RootRange], offset_dwords: u32) -> Option<(u32, u32, u32)> {
    if offset_dwords % 8 != 0 {
        return None;
    }
    let index = u64::from(offset_dwords / 8);
    let mut end = 0u64;
    let mut found = None;
    for range in ranges {
        let start = if range.offset_in_table == u32::MAX {
            end
        } else {
            u64::from(range.offset_in_table)
        };
        end = start + u64::from(range.num);
        if (start..end).contains(&index) {
            let register = range.base_register.checked_add((index - start) as u32)?;
            if found
                .replace((range.range_type, register, range.space))
                .is_some()
            {
                return None;
            }
        }
    }
    found
}
