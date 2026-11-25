//! Cross-check the mesh program's physical FSSM slots and shader bindings.
use crate::{
    Dxbc,
    compute_table::{records, user_data_records},
    rootsig::{RootParamKind, RootRange, RootSig},
    xdat,
};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy)]
pub struct MeshTableBinding {
    pub parameter: usize,
    pub kind: u32,
    pub register: u32,
    pub space: u32,
    pub stride_words: Option<u32>,
}

pub struct MeshLayout {
    pub tables: BTreeMap<(u8, u32), MeshTableBinding>,
    pub raw: BTreeMap<u8, (u32, u32)>,
    pub constants: BTreeMap<u8, (u32, u32, u32)>,
    pub replacements: BTreeMap<usize, RootParamKind>,
}

impl MeshLayout {
    pub fn from_shader(bytes: &[u8], root: &RootSig) -> Option<Self> {
        let container = Dxbc::parse(bytes).ok()?;
        let chunk = |tag| {
            let part = container.chunk(tag)?;
            bytes.get(part.offset..part.offset + part.size)
        };
        let dxil = chunk(b"DXIL")?;
        if word(dxil, 0)? >> 16 != 13 || dxil.get(8..12)? != b"DXIL" {
            return None;
        }
        let [program] = xdat::programs(dxil.get(24..)?).ok()?.try_into().ok()?;
        let resources = shader_resources(bytes)?;
        Self::read(&program, &resources, root)
    }

    pub fn read(program: &[u8], resources: &[[u32; 4]], root: &RootSig) -> Option<Self> {
        let used = records(program)?;
        let user: BTreeSet<_> = user_data_records(program)?.collect();
        if used.len() > 128 || resources.len() > 128 {
            return None;
        }
        let mut expanded = BTreeSet::new();
        for &[ty, space, lower, upper] in resources {
            let count = upper.checked_sub(lower)?.checked_add(1)?;
            if count > 128 || expanded.len() + count as usize > 128 { return None; }
            for register in lower..=upper {
                if !expanded.insert((ty, space, register)) { return None; }
            }
        }
        if used.len() != expanded.len() { return None; }
        let mut result = Self {
            tables: BTreeMap::new(),
            raw: BTreeMap::new(),
            constants: BTreeMap::new(),
            replacements: BTreeMap::new(),
        };
        let mut ranges = BTreeMap::<usize, Vec<RootRange>>::new();
        let mut used_resources = BTreeSet::new();
        let mut sgprs = BTreeMap::new();
        let mut parameter_sgprs = BTreeMap::new();
        for [class, offset, register, shape, location] in used {
            if class == 0x401 {
                // A static sampler: the console inlines its state in the code.
                let mut named = resources.iter().filter(|r| (r[0], r[2], r[3]) == (1, register, register));
                let space = named.next()?[1];
                if named.next().is_some() || !used_resources.insert((1, space, register)) {
                    return None;
                }
                continue;
            }
            let index = (location >> 16) as usize;
            let root_param = root.params.get(index)?;
            let sgpr = ((location >> 8) as u8).checked_add(8)?;
            if sgpr >= 104
                || !matches!(root_param.visibility, 0 | 7)
                || parameter_sgprs
                    .insert(index, sgpr)
                    .is_some_and(|old| old != sgpr)
            {
                return None;
            }
            let named = |ty| resources.iter().filter(|r| (r[0], r[2], r[3]) == (ty, register, register)).count();
            let (ty, width) = match (class, location & 255, shape) {
                (0x300, 4, 1) => (4, 1),
                // Four elements: a typed buffer, or a structured one with one-word elements.
                (0x300, 4, 4) if named(3) == 1 && named(5) == 0 => (3, 1),
                // An eight-word descriptor is a texture.
                (0x300, 8, _) => (3, 1),
                (0x300, 4, n) if (4..=2048).contains(&n) && n % 4 == 0 => (5, 1),
                (0x302, 4, 0) => (2, 1),
                (0x100, 4, 1) => (4, 2),
                (2, 4, 0) => {
                    let RootParamKind::Constants { num32, .. } = root_param.kind else {
                        return None;
                    };
                    if !(1..=4).contains(&num32) {
                        return None;
                    }
                    (2, num32)
                }
                _ => return None,
            };
            let mut matching = expanded.iter().filter(|&&(t, _, reg)| t == ty && reg == register);
            let space = matching.next()?.1;
            let binding_class = if ty == 2 { 2 } else { 0 };
            if matching.next().is_some()
                || !used_resources.insert((binding_class, space, register))
                || u32::from(sgpr) + width > 104
            {
                return None;
            }
            for reg in sgpr..sgpr + width as u8 {
                if sgprs.insert(reg, index).is_some_and(|old| old != index) {
                    return None;
                }
            }
            match &root_param.kind {
                RootParamKind::Table(_)
                    if matches!(class, 0x300 | 0x302)
                        && offset % 8 == 0
                        && user.contains(&[3, offset, 1, register, location >> 8]) =>
                {
                    let kind = if class == 0x302 { 2 } else { 0 };
                    let binding = MeshTableBinding {
                        parameter: index,
                        kind,
                        register,
                        space,
                        stride_words: (ty == 5).then_some(shape / 4),
                    };
                    if result
                        .tables
                        .insert((sgpr, offset.checked_mul(4)?), binding)
                        .is_some()
                    {
                        return None;
                    }
                    ranges.entry(index).or_default().push(RootRange {
                        range_type: kind,
                        num: 1,
                        base_register: register,
                        space,
                        offset_in_table: offset / 8,
                    });
                }
                RootParamKind::Descriptor {
                    kind: 3,
                    register: original,
                    space: original_space,
                } if class == 0x100
                    && offset == 0
                    && user.contains(&[1, 0, 2, register, location >> 8]) =>
                {
                    if result.raw.insert(sgpr, (register, space)).is_some() {
                        return None;
                    }
                    if (register, space) != (*original, *original_space) {
                        result.replacements.insert(
                            index,
                            RootParamKind::Descriptor {
                                kind: 3,
                                register,
                                space,
                            },
                        );
                    }
                }
                RootParamKind::Constants {
                    register: original,
                    space: original_space,
                    num32,
                } if class == 2
                    && offset == 0
                    && user.contains(&[0, 0, *num32, register, location >> 8]) =>
                {
                    if result
                        .constants
                        .insert(sgpr, (register, space, *num32))
                        .is_some()
                    {
                        return None;
                    }
                    if (register, space) != (*original, *original_space) {
                        result.replacements.insert(
                            index,
                            RootParamKind::Constants {
                                register,
                                space,
                                num32: *num32,
                            },
                        );
                    }
                }
                _ => return None,
            }
        }
        for (index, mut used) in ranges {
            let RootParamKind::Table(original) = &root.params[index].kind else {
                return None;
            };
            used.sort_by_key(|r| r.offset_in_table);
            if used
                .windows(2)
                .any(|p| p[0].offset_in_table == p[1].offset_in_table)
            {
                return None;
            }
            if used.iter().any(|r| {
                table_slot(original, r.offset_in_table)
                    != Some((r.range_type, r.base_register, r.space))
            }) {
                result
                    .replacements
                    .insert(index, RootParamKind::Table(used));
            }
        }
        Some(result)
    }

    pub fn takes_sgpr(&self, sgpr: u8) -> bool {
        self.tables
            .range((sgpr, 0)..=(sgpr, u32::MAX))
            .next()
            .is_some()
            || self.raw.contains_key(&sgpr)
            || sgpr
                .checked_sub(1)
                .is_some_and(|low| self.raw.contains_key(&low))
            || self
                .constants
                .range(..=sgpr)
                .next_back()
                .is_some_and(|(&base, &(_, _, count))| u32::from(sgpr - base) < count)
    }
}

fn table_slot(ranges: &[RootRange], index: u32) -> Option<(u32, u32, u32)> {
    let index = u64::from(index);
    let mut end = 0;
    let mut found = None;
    for r in ranges {
        let start = if r.offset_in_table == u32::MAX {
            end
        } else {
            u64::from(r.offset_in_table)
        };
        end = start + u64::from(r.num);
        if (start..end).contains(&index) {
            let register = r.base_register.checked_add((index - start) as u32)?;
            if found.replace((r.range_type, register, r.space)).is_some() {
                return None;
            }
        }
    }
    found
}

fn word(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(*bytes.get(at..)?.first_chunk()?))
}

/// PSV resource type, register space, and inclusive bounds for each resource.
pub fn shader_resources(bytes: &[u8]) -> Option<Vec<[u32; 4]>> {
    let container = Dxbc::parse(bytes).ok()?;
    let psv = container.chunk(b"PSV0")?;
    let data = bytes.get(psv.offset..psv.offset + psv.size)?;
    let table = data.get(4usize.checked_add(word(data, 0)? as usize)?..)?;
    let count = word(table, 0)? as usize;
    if count == 0 {
        return Some(Vec::new());
    }
    let stride = word(table, 4)? as usize;
    if count > 128 || stride < 16 {
        return None;
    }
    table
        .get(8..)?
        .get(..count.checked_mul(stride)?)?
        .chunks_exact(stride)
        .map(|row| Some([word(row, 0)?, word(row, 4)?, word(row, 8)?, word(row, 12)?]))
        .collect()
}

#[cfg(test)]
#[path = "tests/mesh_table.rs"]
mod tests;
