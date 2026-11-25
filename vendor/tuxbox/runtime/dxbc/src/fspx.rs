//! FSPX native compute sections observed in the Scarlett FidelityFX upscaler.
//! Version 1 stores a directory immediately after its four-word header. Section
//! addresses are absolute byte offsets from the first byte of the FSPX record.

pub struct Fspx<'a> {
    pub config: Option<[u32; 7]>,
    pub code: &'a [u8],
}

fn words<const N: usize>(mut bytes: &[u8]) -> Result<[u32; N], String> {
    let mut values = [0; N];
    for value in &mut values {
        let (field, rest) = bytes
            .split_first_chunk::<4>()
            .ok_or("truncated FSPX record")?;
        *value = u32::from_le_bytes(*field);
        bytes = rest;
    }
    Ok(values)
}

pub fn parse_fspx(body: &[u8]) -> Result<Fspx<'_>, String> {
    let [magic, version, count, total] = words(body)?;
    if magic != u32::from_le_bytes(*b"FSPX") || version != 1 {
        return Err("unrecognized FSPX header".into());
    }
    let file = body.get(..total as usize).ok_or("truncated FSPX image")?;
    let directory_end = (count as usize)
        .checked_mul(16)
        .and_then(|n| n.checked_add(16))
        .ok_or("FSPX directory size overflow")?;
    let directory = file
        .get(16..directory_end)
        .ok_or("truncated FSPX directory")?;

    let mut spans = Vec::with_capacity(directory.len() / 16);
    let mut config = None;
    let mut code = None;
    for descriptor in directory.chunks_exact(16) {
        let [kind, offset, length, _reserved] = words(descriptor)?;
        let start = offset as usize;
        let end = start
            .checked_add(length as usize)
            .ok_or("FSPX section length overflow")?;
        let section = file
            .get(start..end)
            .ok_or("FSPX section outside declared image")?;
        if !section.is_empty() {
            if start < directory_end {
                return Err("FSPX section overlaps directory".into());
            }
            spans.push((start, end));
        }
        match kind {
            0 => {
                if config.replace(words::<7>(section)?).is_some() {
                    return Err("multiple FSPX compute configurations".into());
                }
            }
            6 => {
                if code.replace(section).is_some() {
                    return Err("multiple FSPX code sections".into());
                }
            }
            _ => {}
        }
    }
    spans.sort_unstable();
    if spans.windows(2).any(|pair| pair[0].1 > pair[1].0) {
        return Err("overlapping FSPX sections".into());
    }
    let code = code.ok_or("missing FSPX code section")?;
    if code.is_empty() || code.len() % 4 != 0 {
        return Err("partial dword in FSPX code section".into());
    }
    Ok(Fspx { config, code })
}

/// Compute-stage thread group dimensions from the versioned DXBC PSV0 runtime header.
pub fn compute_threads(psv: &[u8]) -> Option<[u32; 3]> {
    let (length, tail) = psv.split_first_chunk::<4>()?;
    let runtime = tail.get(..u32::from_le_bytes(*length) as usize)?;
    if runtime.get(24) != Some(&5) {
        return None;
    }
    let xyz = words::<3>(runtime.get(36..)?).ok()?;
    (!xyz.contains(&0)).then_some(xyz)
}

/// Expected wave lane count bounds `(min, max)` from DXBC `PSV0` `PSVRuntimeInfo0`.
///
/// Returns `None` when `PSV0` lacks `PSVRuntimeInfo0`, leaves the wave range
/// unconstrained (`(0, 0)` or `(0, u32::MAX)`), or encodes `min > max`.
pub fn wave_lane_range(psv: &[u8]) -> Option<(u32, u32)> {
    let (length, tail) = psv.split_first_chunk::<4>()?;
    let runtime = tail.get(..u32::from_le_bytes(*length) as usize)?;
    let [min, max] = words::<2>(runtime.get(16..)?).ok()?;
    (!(min == 0 && matches!(max, 0 | u32::MAX)) && min <= max).then_some((min, max))
}

/// Extract `[numthreads(x, y, z)]` from a compute DXBC container (`PSV0`, SM4/5 `SHEX`,
/// or Xbox `XDAT`/`FSSM` preamble when `native-bindings` is enabled).
pub fn shader_compute_threads(shader: &[u8]) -> Option<[u32; 3]> {
    let dxbc = crate::Dxbc::parse(shader).ok()?;
    if let Some(psv) = dxbc.chunk(b"PSV0")
        && let Some(body) = shader.get(psv.offset..psv.offset + psv.size)
        && let Some(threads) = compute_threads(body)
    {
        return Some(threads);
    }
    if let Some(shex) = dxbc.chunk(b"SHEX")
        && let Some(body) = shader.get(shex.offset..shex.offset + shex.size)
        && let Some(threads) = shex_compute_threads(body)
    {
        return Some(threads);
    }
    #[cfg(feature = "native-bindings")]
    if let Some(dxil) = dxbc.chunk(b"DXIL")
        && let Some(body) = shader.get(dxil.offset..dxil.offset + dxil.size)
        && let Some(threads) = xdat_compute_threads(body)
    {
        return Some(threads);
    }
    None
}

fn shex_compute_threads(body: &[u8]) -> Option<[u32; 3]> {
    if !body.len().is_multiple_of(4) || body.len() < 8 {
        return None;
    }
    let (chunks, _) = body.as_chunks::<4>();
    let version = u32::from_le_bytes(chunks[0]);
    if version >> 16 != 5 {
        return None;
    }
    let declared = u32::from_le_bytes(chunks[1]) as usize;
    if !(2..=chunks.len()).contains(&declared) {
        return None;
    }
    let mut i = 2;
    while i < declared {
        let token = u32::from_le_bytes(chunks[i]);
        let opcode = token & 0x7ff;
        let (length, min_len) = if opcode == 11 {
            // D3D10_SB_OPCODE_CUSTOMDATA stores its total dword count in the next word.
            (u32::from_le_bytes(*chunks.get(i + 1)?) as usize, 2)
        } else {
            (((token >> 24) & 0x7f) as usize, 1)
        };
        if length < min_len || i.checked_add(length)? > declared {
            return None;
        }
        // D3D10_SB_OPCODE_DCL_THREAD_GROUP (155) carries [token, x, y, z].
        if opcode == 155 && token >> 31 == 0 && length == 4 {
            let xyz = [
                u32::from_le_bytes(chunks[i + 1]),
                u32::from_le_bytes(chunks[i + 2]),
                u32::from_le_bytes(chunks[i + 3]),
            ];
            return (!xyz.contains(&0)).then_some(xyz);
        }
        i += length;
    }
    None
}

#[cfg(feature = "native-bindings")]
fn xdat_compute_threads(dxil: &[u8]) -> Option<[u32; 3]> {
    let header = dxil.get(..24)?;
    let version = u32::from_le_bytes(*header.first_chunk::<4>()?);
    if version >> 16 != 5 || dxil.get(8..12)? != b"DXIL" || dxil.get(24..28)? != b"XDAT" {
        return None;
    }
    let programs = crate::xdat::programs(dxil.get(24..)?).ok()?;
    let [program] = programs.as_slice() else {
        return None;
    };
    let decoded = crate::fssm::decode(program).ok()?;
    let [_, x, y, z] = decoded.preamble;
    let xyz = [x, y, z];
    (!xyz.contains(&0)
        && x.checked_mul(y)
            .and_then(|n| n.checked_mul(z))
            .is_some_and(|n| n <= 1024))
    .then_some(xyz)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn psv_with_wave_range(runtime_len: usize, min: u32, max: u32) -> Vec<u8> {
        let mut psv = (runtime_len as u32).to_le_bytes().to_vec();
        let mut runtime = vec![0u8; runtime_len];
        if runtime_len >= 24 {
            runtime[16..20].copy_from_slice(&min.to_le_bytes());
            runtime[20..24].copy_from_slice(&max.to_le_bytes());
        }
        psv.extend_from_slice(&runtime);
        psv
    }

    #[test]
    fn extracts_non_trivial_wave_lane_bounds_from_psv0() {
        assert_eq!(wave_lane_range(&psv_with_wave_range(52, 0, u32::MAX)), None);
        assert_eq!(wave_lane_range(&psv_with_wave_range(24, 0, 0)), None);
        assert_eq!(wave_lane_range(&psv_with_wave_range(24, 64, 64)), Some((64, 64)));
        assert_eq!(wave_lane_range(&psv_with_wave_range(52, 32, 32)), Some((32, 32)));
        assert_eq!(wave_lane_range(&psv_with_wave_range(52, 32, 64)), Some((32, 64)));
        assert_eq!(wave_lane_range(&psv_with_wave_range(52, 0, 64)), Some((0, 64)));
        assert_eq!(wave_lane_range(&psv_with_wave_range(52, 64, 32)), None);
        assert_eq!(wave_lane_range(&psv_with_wave_range(20, 64, 64)), None);
        assert_eq!(wave_lane_range(&psv_with_wave_range(24, 64, 64)[..20]), None);
    }
    use crate::container::tests::container_with;

    fn dwords(values: &[u32]) -> Vec<u8> {
        values.iter().flat_map(|w| w.to_le_bytes()).collect()
    }

    #[test]
    fn extracts_compute_threads_from_psv0_and_shex() {
        let mut runtime = [0u8; 48];
        runtime[24] = 5;
        runtime[36..40].copy_from_slice(&8u32.to_le_bytes());
        runtime[40..44].copy_from_slice(&4u32.to_le_bytes());
        runtime[44..48].copy_from_slice(&2u32.to_le_bytes());
        let mut psv = 48u32.to_le_bytes().to_vec();
        psv.extend_from_slice(&runtime);
        let psv_dxbc = container_with(&[(b"PSV0", &psv)]);
        assert_eq!(shader_compute_threads(&psv_dxbc), Some([8, 4, 2]));

        // SM 5.0 compute shader with preceding customdata + dcl_thread_group(7, 3, 2).
        let shex = dwords(&[
            0x0005_0050,
            10,
            11,
            4,
            0xdead_beef,
            0xcafe_babe,
            (4 << 24) | 155,
            7,
            3,
            2,
        ]);
        let shex_dxbc = container_with(&[(b"SHEX", &shex)]);
        assert_eq!(shader_compute_threads(&shex_dxbc), Some([7, 3, 2]));

        // Non-compute stage or zero thread dimensions fail closed.
        let mut ps_shex = shex.clone();
        ps_shex[0..4].copy_from_slice(&0x0000_0050u32.to_le_bytes());
        assert_eq!(shader_compute_threads(&container_with(&[(b"SHEX", &ps_shex)])), None);
        let mut zero_shex = shex;
        zero_shex[28..32].copy_from_slice(&0u32.to_le_bytes());
        assert_eq!(shader_compute_threads(&container_with(&[(b"SHEX", &zero_shex)])), None);
        assert_eq!(shader_compute_threads(b"not dxbc"), None);
    }

    #[cfg(feature = "native-bindings")]
    #[test]
    fn falls_back_to_xdat_fssm_preamble_when_psv0_lacks_threads() {
        use miniz_oxide::deflate::compress_to_vec_zlib;

        let mut image = vec![0u8; 0x170];
        let write_u64 = |buf: &mut [u8], at: usize, v: u64| buf[at..at + 8].copy_from_slice(&v.to_le_bytes());
        write_u64(&mut image, 0, 6);
        for slot in 0..12 {
            write_u64(&mut image, 8 + 8 * slot, if slot == 11 { 0x80 } else { 0xdead_beef_dead_beef });
        }
        write_u64(&mut image, 0x80, 0x170);
        write_u64(&mut image, 0x88, 4);
        image[0x160..0x170].copy_from_slice(&dwords(&[0, 64, 2, 1]));
        image.extend_from_slice(&0xbf81_0000u32.to_le_bytes());
        let mut fssm = Vec::from(*b"FSSM");
        fssm.extend_from_slice(&0x101u32.to_le_bytes());
        fssm.extend_from_slice(&(image.len() as u64).to_le_bytes());
        fssm.extend_from_slice(&image);
        let compressed = compress_to_vec_zlib(&fssm, 6);
        let mut section = Vec::from(*b"XBCH");
        section.extend_from_slice(&3u32.to_le_bytes());
        section.extend_from_slice(&(compressed.len() as u64).to_le_bytes());
        section.extend_from_slice(&(fssm.len() as u64).to_le_bytes());
        section.extend_from_slice(&compressed);

        let mut xdat = Vec::from(*b"XDAT");
        xdat.extend_from_slice(&dwords(&[2, 36, 13, 60, section.len() as u32, 7, 60 + section.len().next_multiple_of(4) as u32, 4]));
        xdat.extend_from_slice(&section);
        xdat.resize(xdat.len().next_multiple_of(4), 0);
        xdat.extend_from_slice(b"NOBC");

        let mut dxil = dwords(&[0x50060, ((24 + xdat.len()) / 4) as u32]);
        dxil.extend_from_slice(b"DXIL");
        dxil.extend_from_slice(&dwords(&[0x100, (16 + xdat.len() - 4) as u32, 4]));
        dxil.extend_from_slice(&xdat);

        let dxbc = container_with(&[(b"DXIL", &dxil)]);
        assert_eq!(shader_compute_threads(&dxbc), Some([64, 2, 1]));

        // Non-compute stage ignores FSSM preamble.
        dxil[0..4].copy_from_slice(&0x0000_0060u32.to_le_bytes());
        assert_eq!(shader_compute_threads(&container_with(&[(b"DXIL", &dxil)])), None);
    }
}
