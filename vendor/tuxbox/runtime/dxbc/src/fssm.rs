//! Reader for the observed FSSM 0x101 records in the Frosty and Fortnite shader
//! archives. The outer 16-byte header precedes an image whose offsets are all
//! relative to its own first byte. Its stage directory has twelve 64-bit slots.
//! Each observed record has a single occupied slot pointing at a stage descriptor.

use super::rdna2::{Enc, instructions};

pub struct Program {
    pub isa: Vec<u8>,
    pub isa_offset: usize,
    pub preamble: [u32; 4],
    pub isa_data: Vec<u8>,
    pub entry: Option<String>,
}

const ABSENT: u64 = 0xdead_beef_dead_beef;
const CODE_END: u32 = 0xbf9f_0000;

fn field<const N: usize>(bytes: &[u8], at: usize) -> Result<[u8; N], String> {
    bytes
        .get(at..)
        .and_then(|rest| rest.first_chunk())
        .copied()
        .ok_or_else(|| "truncated FSSM record".into())
}

fn extent(image: &[u8], offset: u64, length: u64) -> Result<&[u8], String> {
    let start = usize::try_from(offset).map_err(|_| "FSSM offset overflow")?;
    let size = usize::try_from(length).map_err(|_| "FSSM length overflow")?;
    image
        .get(start..)
        .and_then(|tail| tail.get(..size))
        .ok_or_else(|| "FSSM section outside declared image".into())
}

fn entry_name(image: &[u8], descriptor: &[u8], layout: u64) -> Result<Option<String>, String> {
    let at = match layout {
        5 => 0x58,
        6 => 0x60,
        _ => return Err(format!("unsupported FSSM image layout {layout}")),
    };
    let offset = u64::from_le_bytes(field(descriptor, at)?);
    let count = u32::from_le_bytes(field(descriptor, at + 8)?);
    let size = u32::from_le_bytes(field(descriptor, at + 12)?);
    if count == 0 {
        return Ok(None);
    }
    if count != 1 {
        return Err(format!("unsupported FSSM entry count {count}"));
    }
    let record = extent(image, offset, u64::from(size))?;
    let length = u16::from_le_bytes(field(record, 18)?);
    let name = extent(record, 20, u64::from(length))?;
    if name.is_empty() {
        return Ok(None);
    }
    let text = std::str::from_utf8(name).map_err(|_| "invalid FSSM entry name")?;
    Ok(Some(text.to_owned()))
}

/// Follow direct control flow to distinguish executable words from lookup
/// tables following an endpgm. A branch can lead to a block physically after
/// an early endpgm, including a block that jumps backwards to rejoin the shader.
pub fn executable_extent(program: &[u8]) -> Result<usize, String> {
    // Distinguish an instruction start from its extension words when different
    // control-flow paths meet. Each reachable instruction is decoded once.
    let mut visited = vec![0u8; program.len() / 4];
    let mut pending = vec![0usize];
    let mut furthest = 0;
    let mut ends_in_transfer = false;
    let mut terminated = false;
    while let Some(start) = pending.pop() {
        let mut block = instructions(&program[start..]);
        loop {
            let ins = block
                .next()
                .ok_or("shader control flow falls past the code region")?;
            if !ins.is_complete() {
                return Err("truncated shader instruction".into());
            }
            let at = start + ins.offset;
            let end = at + ins.dwords * 4;
            let marks = visited
                .get_mut(at / 4..end / 4)
                .ok_or("truncated shader instruction")?;
            let (head, operands) = marks.split_first_mut().ok_or("empty shader instruction")?;
            if *head == 1 {
                break;
            }
            if *head != 0 || operands.iter().any(|&mark| mark != 0) {
                return Err("shader branch enters an instruction operand".into());
            }
            *head = 1;
            operands.fill(2);
            if end > furthest {
                furthest = end;
                ends_in_transfer =
                    matches!((ins.enc, ins.op), (Enc::Sopp, 2) | (Enc::Sop1, 32 | 34));
            }
            match (ins.enc, ins.op) {
                (Enc::Sopp, 1 | 27 | 30) => {
                    terminated = true;
                    break;
                }
                (Enc::Sopp, 31) => return Err("shader control flow enters code-end padding".into()),
                (Enc::Sopp, 2 | 4..=9 | 23..=26) | (Enc::Sopk, 22 | 27 | 28) => {
                    let displacement = i16::from_le_bytes(field(program, at)?);
                    let target = end
                        .checked_add_signed(isize::from(displacement) * 4)
                        .filter(|&target| target < program.len())
                        .ok_or("shader branch out of bounds")?;
                    pending.push(target);
                    if matches!((ins.enc, ins.op), (Enc::Sopp, 2)) {
                        break;
                    }
                }
                (Enc::Sop1, 32 | 34) => break, // indirect setpc/return
                _ => {}
            }
        }
    }
    // Some FSSM producers mark the end of a block that transfers control
    // with one adjacent s_endpgm. It is part of the code span, including on
    // export stages that return indirectly and have no fallthrough path.
    if ends_in_transfer && program[furthest..].starts_with(&0xbf81_0000u32.to_le_bytes()) {
        return Ok(furthest + 4);
    }
    if !terminated {
        return Err("shader stage has no reachable endpgm; external linkage unsupported".into());
    }
    Ok(furthest)
}

/// Native LDS allocation in bytes for the layout-6 compute stage (directory
/// slot 11). Other stage descriptors have different layouts.
pub fn compute_lds_bytes(program: &[u8]) -> Option<u32> {
    if program.get(..8)? != b"FSSM\x01\x01\0\0" {
        return None;
    }
    let image = program.get(16..)?;
    if u64::from_le_bytes(field(program, 8).ok()?) != image.len() as u64
        || u64::from_le_bytes(field(image, 0).ok()?) != 6
    {
        return None;
    }
    for slot in image.get(8..96)?.chunks_exact(8) {
        if u64::from_le_bytes(slot.try_into().ok()?) != ABSENT {
            return None;
        }
    }
    let offset = usize::try_from(u64::from_le_bytes(field(image, 96).ok()?)).ok()?;
    let size = u32::from_le_bytes(field(image, offset.checked_add(0x4c)?).ok()?);
    (size <= 32768 && size % 4 == 0).then_some(size)
}

pub fn decode(fssm: &[u8]) -> Result<Program, String> {
    if field::<4>(fssm, 0)? != *b"FSSM" || u32::from_le_bytes(field(fssm, 4)?) != 0x101 {
        return Err("unrecognized FSSM header".into());
    }
    let image = extent(fssm, 16, u64::from_le_bytes(field(fssm, 8)?))?;
    let layout = u64::from_le_bytes(field(image, 0)?);
    let slots = extent(image, 8, 12 * 8)?;
    let mut descriptor_offset = None;
    for slot in slots.chunks_exact(8) {
        let offset = u64::from_le_bytes(field(slot, 0)?);
        if offset != ABSENT && descriptor_offset.replace(offset).is_some() {
            return Err("multiple FSSM stages in one program".into());
        }
    }
    let offset = descriptor_offset.ok_or("missing FSSM stage descriptor")?;
    let descriptor = image
        .get(usize::try_from(offset).map_err(|_| "FSSM offset overflow")?..)
        .ok_or("FSSM stage descriptor out of bounds")?;
    let code_offset = u64::from_le_bytes(field(descriptor, 0)?);
    let capacity = u64::from_le_bytes(field(descriptor, 8)?);
    let mut code = extent(image, code_offset, capacity)?;
    if code.len() % 4 != 0 {
        return Err("partial dword in FSSM code region".into());
    }
    while code.ends_with(&CODE_END.to_le_bytes()) {
        code = &code[..code.len() - 4];
    }
    let isa_bytes = executable_extent(code)?;

    let isa_offset = 16 + code_offset as usize;
    let mut preamble = [0; 4];
    for (index, word) in preamble.iter_mut().enumerate() {
        *word = u32::from_le_bytes(field(fssm, isa_offset - 16 + index * 4)?);
    }
    Ok(Program {
        isa: code[..isa_bytes].to_vec(),
        isa_offset,
        preamble,
        isa_data: code.to_vec(),
        entry: entry_name(image, descriptor, layout)?,
    })
}

#[cfg(test)]
#[path = "tests/fssm.rs"]
mod tests;
