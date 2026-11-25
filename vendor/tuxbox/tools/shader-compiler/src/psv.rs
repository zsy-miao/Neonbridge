//! Shader stage and binding metadata from the DirectX pipeline validation part.
//! Layouts follow [Microsoft's versioned PSV definitions](https://github.com/microsoft/DirectXShaderCompiler/blob/main/include/dxc/DxilContainer/DxilPipelineStateValidation.h).

#[derive(Clone, Copy, Debug)]
pub struct GsInfo {
    pub input_primitive: u32,
    pub output_topology: u32,
    pub max_vertex_count: u16,
}

#[derive(Clone, Copy, Debug)]
pub struct MeshInfo {
    pub vertices: u16,
    pub primitives: u16,
    pub topology: u8,
    pub threads: [u32; 3],
    pub payload: u32,
    pub shared_bytes: u32,
}

#[derive(Clone, Debug)]
pub struct PsvResource {
    pub res_type: u32,
    pub space: u32,
    pub lower: u32,
    pub upper: u32,
}

fn word(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(*bytes.get(offset..)?.first_chunk()?))
}

// Later PSV versions append fields to the runtime info. The binding table
// starts at the declared end even when the producer uses a newer version.
fn sections(body: &[u8]) -> Option<(&[u8], &[u8])> {
    let size = word(body, 0)? as usize;
    if size < 24 {
        return None;
    }
    body.get(4..)?.split_at_checked(size)
}

fn thread_group(runtime: &[u8]) -> Option<[u32; 3]> {
    let xyz = [word(runtime, 36)?, word(runtime, 40)?, word(runtime, 44)?];
    (!xyz.contains(&0)).then_some(xyz)
}

pub use tuxbox_dxbc::compute_threads as parse_compute_threads;

pub fn parse_psv0_gs(body: &[u8]) -> Option<GsInfo> {
    let (runtime, _) = sections(body)?;
    if runtime.get(24) != Some(&2) {
        return None;
    }
    Some(GsInfo {
        input_primitive: word(runtime, 0)?,
        output_topology: word(runtime, 4)?,
        max_vertex_count: u16::from_le_bytes(*runtime.get(26..)?.first_chunk()?),
    })
}

pub fn parse_mesh_info(body: &[u8]) -> Option<MeshInfo> {
    let (runtime, _) = sections(body)?;
    if runtime.get(24) != Some(&13) {
        return None;
    }
    Some(MeshInfo {
        vertices: u16::from_le_bytes(*runtime.get(12..)?.first_chunk()?),
        primitives: u16::from_le_bytes(*runtime.get(14..)?.first_chunk()?),
        topology: *runtime.get(27)?,
        threads: thread_group(runtime)?,
        payload: word(runtime, 8)?,
        shared_bytes: word(runtime, 0)?,
    })
}

pub fn parse_psv0(body: &[u8]) -> Result<Vec<PsvResource>, String> {
    let (_, table) = sections(body).ok_or("invalid PSV runtime info length")?;
    let count = word(table, 0).ok_or("missing PSV resource count")? as usize;
    if count == 0 {
        return Ok(Vec::new());
    }
    let stride = word(table, 4)
        .filter(|&n| n >= 16)
        .ok_or("invalid PSV binding stride")? as usize;
    let bytes = count
        .checked_mul(stride)
        .ok_or("PSV binding table size overflow")?;
    let records = table
        .get(8..)
        .and_then(|data| data.get(..bytes))
        .ok_or("truncated PSV binding table")?;
    records
        .chunks_exact(stride)
        .map(|row| {
            let fields = row
                .first_chunk::<16>()
                .ok_or("truncated PSV binding")?
                .as_chunks::<4>()
                .0;
            Ok(PsvResource {
                res_type: u32::from_le_bytes(fields[0]),
                space: u32::from_le_bytes(fields[1]),
                lower: u32::from_le_bytes(fields[2]),
                upper: u32::from_le_bytes(fields[3]),
            })
        })
        .collect()
}

/// Recover interpolation for portable pixel bytecode, not native Xbox ISA.
/// Layout: Microsoft DXC, DxilPipelineStateValidation.h, PSVSignatureElement0.
/// Cross-check physical rows against ISG1/ISGN instead of guessing qualifiers.
pub fn portable_pixel_interp(code: &[u8]) -> Option<Vec<(String, String)>> {
    portable_pixel_interface(code).map(|interface| interface.interpolation)
}

pub struct PortablePixelInterface {
    pub interpolation: Vec<(String, String)>,
    pub inputs: Vec<tuxbox_dxbc::SigElement>,
}

/// Return the validated input signature alongside its interpolation. The
/// linker must use both to preserve the raw 32-bit console exports.
pub fn portable_pixel_interface(code: &[u8]) -> Option<PortablePixelInterface> {
    use tuxbox_dxbc::{Dxbc, parse_signature_part};
    let container = Dxbc::parse(code).ok()?;
    let part = |tag| {
        let p = container.chunk(tag)?;
        code.get(p.offset..p.offset.checked_add(p.size)?)
    };
    let dxil = part(b"DXIL")?;
    let version = word(dxil, 0)?;
    if version >> 16 != 0 || version & 0xf0 != 0x60 || dxil.get(8..12)? != b"DXIL"
        || (word(dxil, 4)? as usize).checked_mul(4)? != dxil.len() { return None; }
    let start = 8usize.checked_add(word(dxil, 16)? as usize)?;
    let end = start.checked_add(word(dxil, 20)? as usize)?;
    if start < 24 || !dxil.get(start..end)?.starts_with(b"BC\xc0\xde") { return None; }
    let tag = if part(b"ISG1").is_some() { b"ISG1" } else { b"ISGN" };
    let input = parse_signature_part(tag, part(tag)?).ok()?;
    let (runtime, mut data) = sections(part(b"PSV0")?)?;
    if runtime.len() < 36 || runtime[24] != 0 { return None; }
    let count = word(data, 0)? as usize;
    data = data.get(4..)?;
    if count != 0 {
        let stride = word(data, 0)? as usize;
        if stride < 16 { return None; }
        data = data.get(4usize.checked_add(count.checked_mul(stride)?)? ..)?;
    }
    let strings_size = word(data, 0)? as usize;
    if strings_size % 4 != 0 { return None; }
    let (strings, rest) = data.get(4..)?.split_at_checked(strings_size)?;
    let index_count = word(rest, 0)? as usize;
    let (indexes, rest) = rest.get(4..)?.split_at_checked(index_count.checked_mul(4)?)?;
    let elements = usize::from(runtime[28]);
    let total_elements = elements + usize::from(runtime[29]) + usize::from(runtime[30]);
    if total_elements == 0 {
        return input.is_empty().then_some(PortablePixelInterface { interpolation: Vec::new(), inputs: input });
    }
    let stride = word(rest, 0)? as usize;
    if stride < 16 { return None; }
    let records = rest.get(4..)?.get(..total_elements.checked_mul(stride)?)?;
    let mut found = std::collections::BTreeSet::new();
    let mut result = Vec::new();
    for row in records.chunks_exact(stride).take(elements) {
        let rows = usize::from(row[8]);
        let cols = row[10] & 15;
        let col = (row[10] >> 4) & 3;
        if rows == 0 || cols == 0 || cols + col > 4 { return None; }
        // Unallocated system values are not interpolated by the producer.
        if row[10] & 0x40 == 0 {
            if row[11] == 0 { return None; }
            continue;
        }
        let mask = (((1u16 << cols) - 1) << col) as u8;
        let name = strings.get(word(row, 0)? as usize..)?;
        let name = std::str::from_utf8(name.get(..name.iter().position(|&c| c == 0)?)?).ok()?;
        let index = word(row, 4)? as usize;
        for n in 0..rows {
            let reg = u32::from(row[9]).checked_add(n as u32)?;
            // System-generated names may be omitted from the PSV string table.
            let matches: Vec<_> = input.iter().enumerate().filter(|(_, e)| {
                e.register == reg && e.mask == mask && if row[11] == 0 {
                    e.sysval == 0 && e.name.eq_ignore_ascii_case(name)
                } else { e.sysval != 0 }
            }).collect();
            let [(i, e)] = matches.as_slice() else { return None };
            if row[11] == 0 && e.index != word(indexes, index.checked_add(n)?.checked_mul(4)?)? { return None; }
            if !found.insert(*i) { return None; }
            if e.sysval == 1 { continue; } // SV_Position has its own producer declaration.
            let mode = match row[13] {
                1 => "nointerpolation",
                2 => "linear",
                3 => "centroid",
                4 => "noperspective",
                5 => "noperspective centroid",
                6 => "sample",
                7 => "noperspective sample",
                0 if e.sysval != 0 => continue,
                _ => return None,
            };
            result.push((format!("{}{}", e.name, e.index), mode.to_owned()));
        }
    }
    input.iter().enumerate().all(|(i, e)| (e.sysval != 0 && e.register == u32::MAX) || found.contains(&i))
        .then_some(PortablePixelInterface { interpolation: result, inputs: input })
}

#[cfg(test)]
#[path = "tests/psv.rs"]
mod tests;
