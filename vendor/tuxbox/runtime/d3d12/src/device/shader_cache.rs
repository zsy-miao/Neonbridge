//! Prepared desktop replacements for stripped Xbox shader containers.
//! The host publishes a validated translation under the SHA-256 of all bytes
//! in the original guest shader. Portable input never opens the cache.
use std::collections::HashSet;
mod compile;

use std::fmt::Write as _;
use std::fs::File;
use std::io::Read;
use std::sync::{LazyLock, Mutex, Once};
use windows::Win32::Foundation::{E_INVALIDARG, E_NOTIMPL};
use windows::Win32::Security::Cryptography::{BCRYPT_SHA256_ALG_HANDLE, BCryptHash};
use windows::core::HRESULT;

const MAX_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Stage {
    Pixel,
    Vertex,
    Geometry,
    Library,
    Compute,
    Mesh,
    Amplification,
}

impl Stage {
    fn report(self, source: &str, hit: bool) {
        static PIXEL: Once = Once::new();
        static VERTEX: Once = Once::new();
        static GEOMETRY: Once = Once::new();
        static LIBRARY: Once = Once::new();
        static COMPUTE: Once = Once::new();
        static MESH: Once = Once::new();
        static AMPLIFICATION: Once = Once::new();
        let (name, first_hit) = match self {
            Self::Library => ("ray library", &LIBRARY),
            Self::Pixel => ("pixel", &PIXEL),
            Self::Vertex => ("vertex", &VERTEX),
            Self::Geometry => ("geometry", &GEOMETRY),
            Self::Compute => ("compute", &COMPUTE),
            Self::Mesh => ("mesh", &MESH),
            Self::Amplification => ("amplification", &AMPLIFICATION),
        };
        static TRACE: LazyLock<bool> =
            LazyLock::new(|| std::env::var("TUXBOX_TRACE_SHADERS").as_deref() == Ok("1"));
        if *TRACE {
            static SEEN: LazyLock<Mutex<HashSet<String>>> =
                LazyLock::new(|| Mutex::new(HashSet::new()));
            let mut seen = SEEN.lock().unwrap_or_else(|error| error.into_inner());
            if !seen.contains(source) {
                seen.insert(source.to_owned());
                drop(seen);
                crate::diagnostics::log_once(&Once::new(), || {
                    format!("shader request {name} {source}")
                });
            }
        }
        if hit {
            crate::diagnostics::log_once(first_hit, || {
                format!("prepared {name} shader loaded, source SHA-256 {source}")
            });
            return;
        }
        // Parallel pipeline jobs can miss several distinct shaders before the
        // title stops. Record each source once, with bounded process lifetime storage.
        static MISSES: Mutex<Vec<String>> = Mutex::new(Vec::new());
        let mut misses = MISSES.lock().unwrap_or_else(|error| error.into_inner());
        if misses.len() < 64 && !misses.iter().any(|seen| seen == source) {
            misses.push(source.to_owned());
            drop(misses);
            crate::diagnostics::log_once(&Once::new(), || {
                format!("prepared {name} shader missing or invalid, source SHA-256 {source}")
            });
        }
    }
}

#[derive(PartialEq)]
enum ShaderCode {
    Portable,
    Xbox,
}

fn word(data: &[u8], offset: usize) -> Option<usize> {
    Some(u32::from_le_bytes(data.get(offset..offset + 4)?.try_into().ok()?) as usize)
}

fn shader_code(data: &[u8]) -> Option<(Stage, ShaderCode)> {
    if data.len() > MAX_BYTES || !data.starts_with(b"DXBC") || word(data, 24)? != data.len() {
        return None;
    }
    let table_end = 32 + word(data, 28)? * 4;
    let mut program = None;
    let mut has_accc = false;
    for offset in data.get(32..table_end)?.as_chunks::<4>().0 {
        let start = u32::from_le_bytes(*offset) as usize;
        if start < table_end {
            return None;
        }
        let tag = data.get(start..start + 4)?;
        let length = word(data, start + 4)?;
        let body = data.get(start + 8..start + 8 + length)?;
        has_accc |= tag == b"ACCC";
        if tag == b"DXIL" && program.replace(body).is_some() {
            return None;
        }
    }
    let Some(dxil) = program else {
        if !has_accc {
            return None;
        }
        // DXIL-less Scarlett compute programs carry their ISA in ACCC and the
        // stage in PSV0. Validate both before asking the host for a translation.
        let container = tuxbox_dxbc::Dxbc::parse(data).ok()?;
        let unique_part = |tag: &[u8; 4]| {
            let mut parts = container.chunks.iter().filter(|part| &part.fourcc == tag);
            let part = parts.next()?;
            parts
                .next()
                .is_none()
                .then_some(&data[part.offset..part.offset + part.size])
        };
        tuxbox_dxbc::compute_threads(unique_part(b"PSV0")?)?;
        tuxbox_dxbc::parse_fspx(unique_part(b"ACCC")?).ok()?;
        return Some((Stage::Compute, ShaderCode::Xbox));
    };
    // ProgramVersion encodes the shader stage in its upper half and shader
    // model 6.x in its low byte. BitcodeOffset is relative to DXIL magic.
    let version = word(dxil, 0)?;
    let stage = match version >> 16 {
        0 => Stage::Pixel,
        1 => Stage::Vertex,
        2 => Stage::Geometry,
        5 => Stage::Compute,
        6 => Stage::Library,
        13 => Stage::Mesh,
        14 => Stage::Amplification,
        _ => return None,
    };
    if version & 0xf0 != 0x60 || word(dxil, 4)? * 4 != dxil.len() || dxil.get(8..12)? != b"DXIL" {
        return None;
    }
    let offset = 8 + word(dxil, 16)?;
    if offset < 24 {
        return None;
    }
    let bitcode = dxil.get(offset..offset + word(dxil, 20)?)?;
    if bitcode.starts_with(b"BC\xc0\xde") {
        Some((stage, ShaderCode::Portable))
    } else if dxil.get(24..28)? == b"XDAT" && bitcode == b"NOBC" {
        Some((stage, ShaderCode::Xbox))
    } else {
        None
    }
}

fn read_portable(directory: &str, source: &str, stage: Stage) -> Option<Vec<u8>> {
    let path = std::env::temp_dir()
        .join("tuxbox_shader_cache")
        .join(directory)
        .join(format!("{source}.cso"));
    read_portable_file(&path, stage)
}

fn read_portable_file(path: &std::path::Path, stage: Stage) -> Option<Vec<u8>> {
    let file = File::open(path).ok()?;
    let metadata = file.metadata().ok()?;
    if !metadata.is_file() || metadata.len() > MAX_BYTES as u64 {
        return None;
    }
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(metadata.len() as usize).ok()?;
    // A concurrent change must not turn a bounded cache lookup into an
    // unbounded read. The host normally publishes by atomic replacement.
    file.take((MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .ok()?;
    (shader_code(&bytes) == Some((stage, ShaderCode::Portable))).then_some(bytes)
}

pub(super) fn source_digest(shader: &[u8]) -> Option<String> {
    if shader.len() > MAX_BYTES {
        return None;
    }
    let mut digest = [0u8; 32];
    // SAFETY: the system SHA-256 pseudo-handle takes no secret, and the
    // bounded input and 32-byte output remain live through this call.
    unsafe { BCryptHash(BCRYPT_SHA256_ALG_HANDLE, None, shader, &mut digest) }
        .ok()
        .ok()?;
    let mut source = String::with_capacity(64);
    for byte in digest {
        write!(source, "{byte:02x}").ok()?;
    }
    Some(source)
}

pub(super) fn linked_vertex(vertex: &str, pixel: &str) -> Option<Vec<u8>> {
    read_portable("by_vs_ps", &format!("{vertex}_{pixel}"), Stage::Vertex)
}

pub(super) fn linked_geometry(geometry: &str, pixel: &str) -> Option<Vec<u8>> {
    read_portable("by_gs_ps", &format!("{geometry}_{pixel}"), Stage::Geometry)
}

pub(super) fn link_on_demand(
    producer: (&str, &[u8]),
    pixel: (&str, &[u8]),
    producer_dxil: &[u8],
    pixel_dxil: &[u8],
) -> Option<Vec<u8>> {
    if !tuxbox_dxbc::has_packing_mismatch(producer_dxil, pixel_dxil) {
        return None;
    }
    let (stage @ (Stage::Vertex | Stage::Geometry), ShaderCode::Portable) =
        shader_code(producer_dxil)?
    else {
        return None;
    };
    compile::linked(producer, pixel, pixel_dxil, stage)
}

pub(super) fn stencil_target(shader: &[u8], slot: u32) -> Option<Vec<u8>> {
    let source = source_digest(shader)?;
    compile::stencil(shader, &source, slot)
}

pub(super) fn portable_library(bytes: &[u8]) -> bool {
    shader_code(bytes) == Some((Stage::Library, ShaderCode::Portable))
}

pub(super) fn ray_library(bytes: &[u8], source: &str) -> Option<Vec<u8>> {
    let library = read_portable("by_source", source, Stage::Library)
        .or_else(|| compile::on_demand(bytes, source, Stage::Library));
    Stage::Library.report(source, library.is_some());
    library
}

pub(super) fn prepared(shader: &[u8]) -> Option<Vec<u8>> {
    let (stage, ShaderCode::Xbox) = shader_code(shader)? else {
        return None;
    };
    translate(shader, stage)
}

/// A shader may receive its root signature only at pipeline creation. Include
/// the actual signature in the compiler request and cache key for that variant.
pub(super) fn compute_with_root(shader: &[u8], root: &[u8]) -> Option<Vec<u8>> {
    let (Stage::Compute, ShaderCode::Xbox) = shader_code(shader)? else { return None };
    translate(&with_root(shader, root)?, Stage::Compute)
}

pub(super) fn raster_with_root(shader: &[u8], root: &[u8]) -> Option<Vec<u8>> {
    let (Stage::Pixel | Stage::Vertex, ShaderCode::Xbox) = shader_code(shader)? else { return None };
    with_root(shader, root)
}

fn with_root(shader: &[u8], root: &[u8]) -> Option<Vec<u8>> {
    use tuxbox_dxbc::Dxbc;
    let dxbc = Dxbc::parse(shader).ok()?;
    if dxbc.chunk(b"RTS0").is_some() || root.is_empty() { return None; }
    let count = dxbc.chunks.len();
    let end = 32 + count * 4;
    let root_offset = (shader.len() + 4).next_multiple_of(4);
    let size = root_offset.checked_add(8)?.checked_add(root.len())?;
    if size > MAX_BYTES { return None; }
    let mut input = Vec::with_capacity(size);
    input.extend_from_slice(&shader[..24]);
    input[4..20].fill(0);
    input.extend_from_slice(&(size as u32).to_le_bytes());
    input.extend_from_slice(&((count + 1) as u32).to_le_bytes());
    for part in &dxbc.chunks {
        input.extend_from_slice(&((part.offset - 8 + 4) as u32).to_le_bytes());
    }
    input.extend_from_slice(&(root_offset as u32).to_le_bytes());
    input.extend_from_slice(&shader[end..]);
    input.resize(root_offset, 0);
    input.extend_from_slice(b"RTS0");
    input.extend_from_slice(&(root.len() as u32).to_le_bytes());
    input.extend_from_slice(root);
    Some(input)
}

/// Console mesh stages always use a DXIL container. Resolve stripped programs
/// before the host call and require portable replacements to match the stage.
pub(super) fn mesh_stage(shader: &[u8], expected: Stage) -> Result<Option<Vec<u8>>, HRESULT> {
    match shader_code(shader) {
        Some((stage, ShaderCode::Portable)) if stage == expected => Ok(None),
        Some((stage, ShaderCode::Xbox)) if stage == expected => {
            translate(shader, stage).map(Some).ok_or(E_NOTIMPL)
        }
        _ => Err(E_INVALIDARG),
    }
}

fn translate(shader: &[u8], stage: Stage) -> Option<Vec<u8>> {
    let source = source_digest(shader)?;
    let translation = read_portable("by_source", &source, stage)
        .or_else(|| compile::on_demand(shader, &source, stage));
    stage.report(&source, translation.is_some());
    translation
}
