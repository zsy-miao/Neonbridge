//! Shader interface records from the DirectX container's input and output parts.
//!
//! [Microsoft's record declarations](https://github.com/microsoft/DirectXShaderCompiler/blob/main/projects/dxilconv/include/Support/DxbcSignatures.h)
//! define the three layouts. [LLVM's program signature documentation](https://llvm.org/docs/DirectX/DXContainer.html#program-signature-sg1-parts)
//! describes how the parameter table and semantic names are addressed.

use crate::Dxbc;
use crate::container::{take, word};

// Each record owns its name even when several records share a string offset.
const MAX_DECODED_BYTES: usize = 64 << 20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SigElement {
    pub name: String,
    pub index: u32,
    pub sysval: u32,
    pub comp_type: u32,
    pub register: u32,
    pub mask: u8,
    pub rw_mask: u8,
}

enum Layout {
    Legacy,
    WithStream,
    WithStreamAndPrecision,
}

/// Reads the payload of an ISG1 or OSG1 signature part.
///
/// # Errors
///
/// Returns an error for a truncated table, an invalid semantic name, or
/// records and owned names exceeding 64 MiB in total.
pub fn parse_signature(body: &[u8]) -> Result<Vec<SigElement>, String> {
    read_parameters(body, Layout::WithStreamAndPrecision, MAX_DECODED_BYTES)
}

/// Reads shader signature records in their original declaration order.
pub fn parse_signature_part(tag: &[u8; 4], body: &[u8]) -> Result<Vec<SigElement>, String> {
    match tag {
        b"ISGN" | b"OSGN" => read_parameters(body, Layout::Legacy, MAX_DECODED_BYTES),
        b"OSG5" => read_parameters(body, Layout::WithStream, MAX_DECODED_BYTES),
        b"ISG1" | b"OSG1" | b"PSG1" => parse_signature(body),
        _ => Err("unknown DXBC signature part".into()),
    }
}

fn read_parameters(
    body: &[u8],
    layout: Layout,
    byte_limit: usize,
) -> Result<Vec<SigElement>, String> {
    let mut header = body;
    let count = word(&mut header)? as usize;
    let start = word(&mut header)? as usize;
    if start < 8 {
        return Err("signature parameter table overlaps its header".into());
    }
    // The 24-byte common fields are preceded by an optional stream index
    // and followed by a minimum-precision field on 11.1 records.
    let (stride, common_start) = match layout {
        Layout::Legacy => (24, 0),
        Layout::WithStream => (28, 4),
        Layout::WithStreamAndPrecision => (32, 4),
    };
    let end = count
        .checked_mul(stride)
        .and_then(|n| start.checked_add(n))
        .ok_or("signature parameter table size overflow")?;
    let table = body
        .get(start..end)
        .ok_or("truncated signature parameter table")?;
    let mut name_bytes_left = count
        .checked_mul(size_of::<SigElement>())
        .and_then(|bytes| byte_limit.checked_sub(bytes))
        .ok_or("signature records exceed decoded size limit")?;
    let mut parameters = Vec::with_capacity(count);
    for entry in table.chunks_exact(stride) {
        let mut fields = &entry[common_start..];
        let name_offset = word(&mut fields)? as usize;
        if name_offset < end {
            return Err("semantic name overlaps the signature header or parameters".into());
        }
        let tail = body
            .get(name_offset..)
            .ok_or("semantic name offset out of range")?;
        let scan_len = tail.len().min(name_bytes_left.saturating_add(1));
        let name = std::ffi::CStr::from_bytes_until_nul(&tail[..scan_len])
            .map_err(|_| "unterminated semantic name or decoded size limit exceeded")?
            .to_str()
            .map_err(|_| "invalid UTF-8 semantic name")?;
        if name.is_empty() {
            return Err("empty semantic name".into());
        }
        let index = word(&mut fields)?;
        let sysval = word(&mut fields)?;
        let comp_type = word(&mut fields)?;
        let register = word(&mut fields)?;
        let [mask, rw_mask] = take(&mut fields)?;
        name_bytes_left -= name.len();
        parameters.push(SigElement {
            name: name.to_owned(),
            index,
            sysval,
            comp_type,
            register,
            mask,
            rw_mask,
        });
    }
    Ok(parameters)
}

pub(crate) fn signature(code: &[u8], tags: &[&[u8; 4]]) -> Option<Vec<SigElement>> {
    let container = Dxbc::parse(code).ok()?;
    tags.iter().find_map(|tag| {
        let part = container.chunk(tag)?;
        let body = &code[part.offset..part.offset + part.size];
        parse_signature_part(tag, body).ok()
    })
}

#[cfg(test)]
#[path = "tests/signature.rs"]
mod tests;
