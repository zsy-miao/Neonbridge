//! Serialized Direct3D 12 root signatures (RTS0 versions 1.0 and 1.1).
//!
//! Layouts follow the [DXC serialized structures](https://github.com/microsoft/DirectXShaderCompiler/blob/main/include/dxc/DxilRootSignature/DxilRootSignature.h)
//! and [LLVM's RTS0 description](https://llvm.org/docs/DirectX/DXContainer.html#root-signature-rts0-part).
//! All offsets in the payload are byte offsets from the version field.

// Serialized descriptor tables can share ranges. Each decoded table owns a copy.
const MAX_DECODED_BYTES: usize = 64 << 20;

#[derive(Clone, Debug)]
pub struct RootRange {
    pub range_type: u32,
    pub num: u32,
    pub base_register: u32,
    pub space: u32,
    pub offset_in_table: u32,
}

#[derive(Clone, Debug)]
pub enum RootParamKind {
    Table(Vec<RootRange>),
    Constants {
        register: u32,
        space: u32,
        num32: u32,
    },
    Descriptor {
        kind: u32,
        register: u32,
        space: u32,
    },
}

#[derive(Clone, Debug)]
pub struct RootParam {
    pub visibility: u32,
    pub kind: RootParamKind,
}

#[derive(Clone, Debug)]
pub struct RootSig {
    pub version: u32,
    pub flags: u32,
    pub params: Vec<RootParam>,
    pub num_static_samplers: u32,
    pub static_samplers: Vec<[u32; 13]>,
}

fn words<const N: usize>(mut record: &[u8]) -> Result<[u32; N], String> {
    let mut values = [0; N];
    for value in &mut values {
        let (word, rest) = record
            .split_first_chunk::<4>()
            .ok_or("truncated RTS0 record")?;
        *value = u32::from_le_bytes(*word);
        record = rest;
    }
    Ok(values)
}

fn section(body: &[u8], offset: u32, count: u32, stride: usize) -> Result<&[u8], String> {
    if count != 0 && offset < 24 {
        return Err("RTS0 section overlaps its header".into());
    }
    let start = offset as usize;
    let end = (count as usize)
        .checked_mul(stride)
        .and_then(|n| start.checked_add(n))
        .ok_or("RTS0 section size overflow")?;
    body.get(start..end)
        .ok_or_else(|| "RTS0 section out of bounds".into())
}

fn charge_records<T>(remaining: &mut usize, count: u32) -> Result<(), String> {
    *remaining = (count as usize)
        .checked_mul(std::mem::size_of::<T>())
        .and_then(|bytes| remaining.checked_sub(bytes))
        .ok_or("RTS0 records exceed decoded size limit")?;
    Ok(())
}

fn read_table(
    body: &[u8],
    offset: u32,
    v11: bool,
    remaining: &mut usize,
) -> Result<RootParamKind, String> {
    let [count, ranges_offset] = words(section(body, offset, 1, 8)?)?;
    let stride = if v11 { 24 } else { 20 };
    let records = section(body, ranges_offset, count, stride)?;
    charge_records::<RootRange>(remaining, count)?;
    let mut ranges = Vec::with_capacity(count as usize);
    for row in records.chunks_exact(stride) {
        let [range_type, num, base_register, space] = words(row)?;
        // Version 1.1 inserts range flags before the table-relative offset.
        let [offset_in_table] = words(&row[stride - 4..])?;
        ranges.push(RootRange {
            range_type,
            num,
            base_register,
            space,
            offset_in_table,
        });
    }
    Ok(RootParamKind::Table(ranges))
}

/// Reads binding locations, visibility, and the raw words of static samplers.
///
/// # Errors
///
/// Returns an error for an unsupported version or parameter type, a truncated
/// record, a section extending beyond the supplied RTS0 payload, or decoded
/// parameters, ranges and static samplers exceeding 64 MiB in total.
pub fn parse_rts0(body: &[u8]) -> Result<RootSig, String> {
    parse_rts0_with_limit(body, MAX_DECODED_BYTES)
}

fn parse_rts0_with_limit(body: &[u8], mut remaining: usize) -> Result<RootSig, String> {
    let [
        version,
        num_params,
        params_offset,
        num_static_samplers,
        sampler_offset,
        flags,
    ] = words(body)?;
    let v11 = match version {
        1 => false,
        2 => true,
        _ => return Err(format!("unsupported RTS0 version {version}")),
    };
    let headers = section(body, params_offset, num_params, 12)?;
    let samplers = section(body, sampler_offset, num_static_samplers, 52)?;
    charge_records::<RootParam>(&mut remaining, num_params)?;
    charge_records::<[u32; 13]>(&mut remaining, num_static_samplers)?;
    let mut params = Vec::with_capacity(num_params as usize);
    for header in headers.chunks_exact(12) {
        let [tag, visibility, offset] = words(header)?;
        let kind = match tag {
            0 => read_table(body, offset, v11, &mut remaining)?,
            1 => {
                let [register, space, num32] = words(section(body, offset, 1, 12)?)?;
                RootParamKind::Constants {
                    register,
                    space,
                    num32,
                }
            }
            2..=4 => {
                // Version 1.1 appends a flags word after the register space.
                let stride = if v11 { 12 } else { 8 };
                let [register, space] = words(section(body, offset, 1, stride)?)?;
                RootParamKind::Descriptor {
                    kind: tag,
                    register,
                    space,
                }
            }
            _ => return Err(format!("unknown RTS0 parameter type {tag}")),
        };
        params.push(RootParam { visibility, kind });
    }
    let mut static_samplers = Vec::with_capacity(num_static_samplers as usize);
    for sampler in samplers.chunks_exact(52) {
        static_samplers.push(words(sampler)?);
    }
    Ok(RootSig {
        version,
        flags,
        params,
        num_static_samplers,
        static_samplers,
    })
}

impl RootSig {
    /// Finite ranges that can address textures from the CBV/SRV/UAV heap.
    /// Unbounded tables and direct resource heap indexing require the full heap.
    pub fn placed_texture_tables(&self) -> Option<Vec<(u32, std::ops::Range<u32>)>> {
        const RESOURCE_HEAP_DIRECTLY_INDEXED: u32 = 0x400;
        if self.flags & RESOURCE_HEAP_DIRECTLY_INDEXED != 0 {
            return None;
        }
        let mut tables = Vec::new();
        for (index, param) in self.params.iter().enumerate() {
            let RootParamKind::Table(ranges) = &param.kind else {
                continue;
            };
            let mut next = 0;
            for range in ranges {
                if range.num == u32::MAX {
                    return None;
                }
                let start = if range.offset_in_table == u32::MAX {
                    next
                } else {
                    range.offset_in_table
                };
                next = start.checked_add(range.num)?;
                match range.range_type {
                    0 | 1 if start < next => tables.push((index as u32, start..next)),
                    0..=3 => {} // SRV, UAV, CBV, sampler.
                    _ => return None,
                }
            }
        }
        Some(tables)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn serialize(words: &[u32]) -> Vec<u8> {
        words.iter().flat_map(|value| value.to_le_bytes()).collect()
    }

    #[test]
    fn texture_spans_resolve_append_and_keep_bindless_conservative() {
        let range = |range_type, num, offset_in_table| RootRange {
            range_type,
            num,
            offset_in_table,
            base_register: 0,
            space: 0,
        };
        let table = |ranges| RootParam {
            visibility: 0,
            kind: RootParamKind::Table(ranges),
        };
        let mut root = RootSig {
            version: 2,
            flags: 0,
            params: vec![
                table(vec![range(3, 1, 0)]),
                table(vec![range(2, 2, 4), range(0, 3, u32::MAX), range(1, 1, 16)]),
            ],
            num_static_samplers: 0,
            static_samplers: vec![],
        };
        assert_eq!(
            root.placed_texture_tables(),
            Some(vec![(1, 6..9), (1, 16..17)])
        );
        root.flags = 0x400;
        assert_eq!(root.placed_texture_tables(), None);
        root.flags = 0;
        root.params.push(table(vec![range(0, u32::MAX, 0)]));
        assert_eq!(root.placed_texture_tables(), None);
    }

    #[test]
    fn resolves_offsets_and_preserves_register_spaces_and_sampler_bits() {
        // Parameter headers follow the static sampler. Descriptor rows and
        // constants appear in a different physical order from their headers.
        for version in [1, 2] {
            let mut data = vec![version, 3, 76, 1, 24, 0x41];
            let sampler = [17, 3, 2, 4, 0xff00_1122, 16, 8, 2, 0, 0x7f80_0000, 7, 5, 4];
            data.extend_from_slice(&sampler);
            data.extend([0, 5, 124, 4, 3, 132, 1, 0, 112]); // headers
            data.extend([9, 8, 6]); // constants
            let first_range = if version == 1 { 140 } else { 144 };
            data.extend([2, first_range]); // table
            data.extend([7, 11]); // root UAV
            if version == 2 {
                data.push(8);
            }
            data.extend([0, u32::MAX, 12, 15]);
            if version == 2 {
                data.push(2);
            }
            data.push(u32::MAX);
            data.extend([2, 7, 19, 23]);
            if version == 2 {
                data.push(4);
            }
            data.push(31);

            let root = parse_rts0(&serialize(&data)).unwrap();
            assert_eq!(
                (root.version, root.flags, root.num_static_samplers),
                (version, 0x41, 1)
            );
            assert_eq!(root.static_samplers, [sampler]);
            let [table, desc, constants] = root.params.as_slice() else {
                panic!("parameter count")
            };
            assert_eq!(table.visibility, 5);
            let RootParamKind::Table(ranges) = &table.kind else {
                panic!("descriptor table")
            };
            assert_eq!(ranges.len(), 2);
            assert_eq!(
                (ranges[0].num, ranges[0].offset_in_table),
                (u32::MAX, u32::MAX)
            );
            let range = &ranges[1];
            assert_eq!(
                (
                    range.range_type,
                    range.num,
                    range.base_register,
                    range.space,
                    range.offset_in_table
                ),
                (2, 7, 19, 23, 31)
            );
            assert!(matches!(
                desc.kind,
                RootParamKind::Descriptor {
                    kind: 4,
                    register: 7,
                    space: 11
                }
            ));
            assert!(matches!(
                constants.kind,
                RootParamKind::Constants {
                    register: 9,
                    space: 8,
                    num32: 6
                }
            ));
        }
    }

    #[test]
    fn counts_each_owned_copy_of_shared_ranges_toward_the_limit() {
        let mut words = vec![1, 2, 24, 1, 76, 0]; // one static sampler
        words.extend([0, 1, 48, 0, 5, 48]); // both headers point to the same table
        words.extend([1, 56, 0, 4, 7, 0, 0]); // one range at offset 56
        words.extend([0; 13]);
        let body = serialize(&words);
        let bytes = 2 * (std::mem::size_of::<RootParam>() + std::mem::size_of::<RootRange>())
            + std::mem::size_of::<[u32; 13]>();
        let root = parse_rts0_with_limit(&body, bytes).unwrap();
        assert_eq!(root.params.len(), 2);
        for param in root.params {
            let RootParamKind::Table(ranges) = param.kind else {
                panic!("expected descriptor table");
            };
            assert_eq!(ranges.len(), 1);
            assert_eq!(ranges[0].base_register, 7);
        }
        assert!(parse_rts0_with_limit(&body, bytes - 1).is_err());
    }

    #[test]
    fn rejects_invalid_offsets_counts_and_short_versioned_records() {
        let valid = [2, 1, 24, 0, 0, 0, 2, 5, 36, 4, 7, 2];
        let bytes = serialize(&valid);
        assert!(parse_rts0(&bytes).is_ok());
        for n in 0..bytes.len() {
            assert!(parse_rts0(&bytes[..n]).is_err(), "accepted prefix {n}");
        }
        for (index, bad) in [
            (0, 3),
            (1, u32::MAX),
            (2, 4),
            (2, u32::MAX),
            (3, u32::MAX),
            (4, u32::MAX),
            (6, 9),
            (8, 16),
            (8, 44),
        ] {
            let mut corrupt = valid;
            corrupt[index] = bad;
            assert!(parse_rts0(&serialize(&corrupt)).is_err(), "field {index}");
        }
        let incomplete_range = [2, 1, 24, 0, 0, 0, 0, 0, 36, 1, 44, 0, 1, 2, 3, 4];
        assert!(parse_rts0(&serialize(&incomplete_range)).is_err());
        assert!(
            parse_rts0(&serialize(&[1, 0, 0, 0, 0, 0]))
                .unwrap()
                .params
                .is_empty()
        );
    }
}
