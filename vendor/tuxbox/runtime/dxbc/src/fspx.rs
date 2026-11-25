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
#[path = "tests/fspx.rs"]
mod tests;
