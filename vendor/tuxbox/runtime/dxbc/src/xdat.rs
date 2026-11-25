//! Native program directory observed in Frosty's cooked global and material DXBCs.
//! XDAT follows a 24-byte DXIL program header. Its directory and section offsets
//! include that header, even though the caller's slice begins at the XDAT magic.

use miniz_oxide::inflate::{
    core::{
        decompress,
        inflate_flags::{TINFL_FLAG_PARSE_ZLIB_HEADER, TINFL_FLAG_USING_NON_WRAPPING_OUTPUT_BUF},
        DecompressorOxide,
    },
    TINFLStatus,
};

const PROGRAM_HEADER_BYTES: usize = 24;
const DIRECTORY_ENTRY_BYTES: usize = 12;
const NATIVE_PROGRAM: u32 = 13;
// Allocation limits for one input, including every variant it contains.
const MAX_SECTIONS: usize = 4096;
const MAX_INFLATED_BYTES: usize = 64 << 20;

fn field<const N: usize>(bytes: &[u8], offset: usize) -> Result<[u8; N], String> {
    bytes
        .get(offset..)
        .and_then(|tail| tail.first_chunk().copied())
        .ok_or_else(|| "truncated XDAT or XBCH field".into())
}

fn xdat_offset(encoded: u32) -> Result<usize, String> {
    (encoded as usize)
        .checked_sub(PROGRAM_HEADER_BYTES)
        .ok_or_else(|| "XDAT offset precedes its header".into())
}

fn inflate_program(section: &[u8], remaining: usize) -> Result<Vec<u8>, String> {
    if !section.starts_with(b"XBCH") || u32::from_le_bytes(field(section, 4)?) != 3 {
        return Err("expected an XBCH version 3 program".into());
    }
    let stored = u64::from_le_bytes(field(section, 8)?);
    let expanded = u64::from_le_bytes(field(section, 16)?);
    let compressed = &section[24..];
    if stored != compressed.len() as u64 {
        return Err("XBCH compressed size differs from its XDAT section".into());
    }
    if !(4..=remaining as u64).contains(&expanded) {
        return Err("XBCH expanded size exceeds the program budget or cannot hold FSSM".into());
    }
    let mut program = vec![0; expanded as usize];
    let flags = TINFL_FLAG_PARSE_ZLIB_HEADER | TINFL_FLAG_USING_NON_WRAPPING_OUTPUT_BUF;
    let (status, consumed, written) = decompress(
        &mut DecompressorOxide::new(),
        compressed,
        &mut program,
        0,
        flags,
    );
    if status != TINFLStatus::Done || consumed != compressed.len() || written != program.len() {
        return Err(format!("incomplete or invalid XBCH zlib stream ({status:?}, consumed {consumed}/{stored}, wrote {written}/{expanded})"));
    }
    if !program.starts_with(b"FSSM") {
        return Err("XBCH program is missing the FSSM signature".into());
    }
    Ok(program)
}

/// Extract every FSSM program in directory order from the XDAT portion of a DXIL chunk.
pub fn programs(xdat: &[u8]) -> Result<Vec<Vec<u8>>, String> {
    if !xdat.starts_with(b"XDAT") {
        return Err("expected XDAT signature".into());
    }
    let count = u32::from_le_bytes(field(xdat, 4)?) as usize;
    if count > MAX_SECTIONS {
        return Err("XDAT section count exceeds the directory limit".into());
    }
    let start = xdat_offset(u32::from_le_bytes(field(xdat, 8)?))?;
    let end = start
        .checked_add(count * DIRECTORY_ENTRY_BYTES)
        .ok_or("XDAT directory size overflow")?;
    if start < 12 || end > xdat.len() {
        return Err("XDAT directory exceeds its container or overlaps the header".into());
    }

    let mut ranges = Vec::with_capacity(count);
    let mut native = Vec::new();
    let (directory, _) = xdat[start..end].as_chunks::<DIRECTORY_ENTRY_BYTES>();
    for entry in directory {
        let kind = u32::from_le_bytes(field(entry, 0)?);
        let offset = xdat_offset(u32::from_le_bytes(field(entry, 4)?))?;
        let length = u32::from_le_bytes(field(entry, 8)?) as usize;
        let limit = offset
            .checked_add(length)
            .ok_or("XDAT section size overflow")?;
        if offset < end || limit > xdat.len() {
            return Err("XDAT section exceeds its container or overlaps the directory".into());
        }
        if length != 0 {
            ranges.push((offset, limit));
        }
        if kind == NATIVE_PROGRAM {
            native.push(&xdat[offset..limit]);
        }
    }
    ranges.sort_unstable();
    if ranges.windows(2).any(|pair| pair[1].0 < pair[0].1) {
        return Err("XDAT section ranges overlap".into());
    }
    if native.is_empty() {
        return Err("XDAT directory has no native program".into());
    }

    let mut remaining = MAX_INFLATED_BYTES;
    let mut programs = Vec::with_capacity(native.len());
    for section in native {
        let program = inflate_program(section, remaining)?;
        remaining -= program.len();
        programs.push(program);
    }
    Ok(programs)
}

#[cfg(test)]
#[path = "tests/xdat.rs"]
pub(crate) mod tests;
