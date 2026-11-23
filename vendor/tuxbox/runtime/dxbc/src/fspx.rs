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
