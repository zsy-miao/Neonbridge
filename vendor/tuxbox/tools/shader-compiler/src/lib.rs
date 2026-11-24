//! Read Xbox shader containers and translate their native programs for desktop D3D12.

mod compute_hlsl;
mod constant_pixel;
use tuxbox_dxbc::fssm;
#[cfg(feature = "legacy-hlsl")]
pub mod hlsl;
#[cfg(feature = "legacy-hlsl")]
pub mod meta;
mod native_compute;
mod psv;
pub use tuxbox_dxbc::rdna2;
pub mod ray;
use tuxbox_dxbc::rootsig;
use tuxbox_dxbc::xdat;

use tuxbox_dxbc::{parse_signature_part, SigElement};
pub use tuxbox_dxbc::{Chunk, Dxbc};

/// Emits a pixel shader whose exports and live samples can be determined statically.
pub fn independent_pixel_hlsl(shader: &ShaderExtract) -> Option<String> {
    constant_pixel::hlsl(shader)
}

/// Whether the lowering needs the runtime's native TLAS root descriptor.
pub fn requires_inline_ray_query(shader: &ShaderExtract) -> bool {
    #[cfg(feature = "legacy-hlsl")]
    if hlsl::radiance_probe::identify(shader).is_some() { return true; }
    compute_hlsl::card_direct_shadow(shader)
}

/// Translate a standalone shader when its complete behavior is supported by TuxBox.
pub fn independent_hlsl(shader: &ShaderExtract) -> Option<String> {
    match shader.stage {
        Stage::Pixel => independent_pixel_hlsl(shader),
        Stage::Compute => compute_hlsl::hlsl(shader),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Pixel,
    Vertex,
    Geometry,
    Hull,
    Domain,
    Compute,
    Mesh,
    Amplification,
    Unknown(u16),
}

impl From<u16> for Stage {
    fn from(kind: u16) -> Self {
        match kind {
            0 => Self::Pixel,
            1 => Self::Vertex,
            2 => Self::Geometry,
            3 => Self::Hull,
            4 => Self::Domain,
            5 => Self::Compute,
            13 => Self::Mesh,
            14 => Self::Amplification,
            other => Self::Unknown(other),
        }
    }
}

/// Native code, launch configuration and Direct3D interface for one stage.
#[derive(Clone, Debug)]
pub struct ShaderExtract {
    pub stage: Stage,
    pub shader_model: (u32, u32),
    /// Feature requirements declared in the container's SFI0 part.
    pub required_features: Option<u64>,
    pub entry: Option<String>,
    pub isa: Vec<u8>,
    /// Includes any constants addressed relative to the program counter.
    pub isa_data: Vec<u8>,
    pub isa_offset_in_fssm: usize,
    pub isa_preamble: [u32; 4],
    pub fssm: Vec<u8>,
    pub bitcode_stripped: bool,
    pub fspx_config: Option<[u32; 7]>,
    pub ps09_pixel_inputs: Option<u32>,
    pub psv_threads: Option<[u32; 3]>,
    pub input_sig: Vec<SigElement>,
    pub output_sig: Vec<SigElement>,
    pub primitive_sig: Vec<SigElement>,
    pub has_input_sig: bool,
    pub has_output_sig: bool,
    pub has_root_sig: bool,
    pub has_psv0: bool,
    pub resources: Vec<psv::PsvResource>,
    pub gs_info: Option<psv::GsInfo>,
    pub mesh_info: Option<psv::MeshInfo>,
    pub root_sig: Option<rootsig::RootSig>,
}

struct NativeProgram {
    stage: Stage,
    sm: (u32, u32),
    bitcode_stripped: bool,
    entry: Option<String>,
    fssm: Vec<u8>,
    isa: Vec<u8>,
    isa_data: Vec<u8>,
    isa_off: usize,
    isa_preamble: [u32; 4],
    fspx_config: Option<[u32; 7]>,
}

fn feeds_rasterizer(code: &[u8]) -> bool {
    use rdna2::{Enc, Opnd};
    for (number, ins) in rdna2::instructions(code).enumerate() {
        match ins.enc {
            // POS0 or PARAM exports. A merged stage can call a separate
            // fetch program through the s[12:13] pair before producing outputs.
            Enc::Exp if ins.aux.imm == 12 || (32..64).contains(&ins.aux.imm) => return true,
            Enc::Sop1
                if number < 16
                    && ins.mnemonic == "s_swappc_b64"
                    && ins.dst == [Opnd::Sgpr(12)]
                    && ins.src == [Opnd::Sgpr(12)] =>
            {
                return true
            }
            _ => {}
        }
    }
    false
}

fn native_dxil(body: &[u8]) -> Result<NativeProgram, String> {
    let (header, _) = body
        .split_at_checked(24)
        .ok_or("truncated DXIL program header")?;
    let fields = header.as_chunks::<4>().0;
    if &fields[2] != b"DXIL" {
        return Err("missing DXIL bitcode header".into());
    }
    let version = u32::from_le_bytes(fields[0]);
    let size = (u32::from_le_bytes(fields[1]) as usize)
        .checked_mul(4)
        .ok_or("invalid DXIL size")?;
    let program = body.get(..size).ok_or("DXIL program exceeds part")?;
    let bitcode = program
        .get(8..)
        .and_then(|p| p.get(u32::from_le_bytes(fields[4]) as usize..))
        .and_then(|p| p.get(..u32::from_le_bytes(fields[5]) as usize))
        .ok_or("invalid DXIL bitcode range")?;
    let stage = Stage::from((version >> 16) as u16);
    let variants = xdat::programs(program.get(24..).ok_or("invalid XDAT program size")?)?;
    let choose_vertex = stage == Stage::Vertex && variants.len() > 1;
    let mut first = None;
    let mut error = String::from("XDAT contains no executable shader");
    for bytes in variants {
        match fssm::decode(&bytes) {
            Ok(decoded) => {
                let preferred = !choose_vertex || feeds_rasterizer(&decoded.isa);
                if preferred || first.is_none() {
                    first = Some((bytes, decoded));
                }
                if preferred {
                    break;
                }
            }
            Err(reason) => error = reason,
        }
    }
    let (fssm, decoded) = first.ok_or(error)?;
    Ok(NativeProgram {
        stage,
        sm: ((version >> 4) & 15, version & 15),
        bitcode_stripped: bitcode == b"NOBC",
        entry: decoded.entry,
        isa: decoded.isa,
        isa_data: decoded.isa_data,
        isa_off: decoded.isa_offset,
        isa_preamble: decoded.preamble,
        fssm,
        fspx_config: None,
    })
}

/// Extracts the native stage and the metadata used to compile and link it.
pub fn extract(bytes: &[u8]) -> Result<ShaderExtract, String> {
    let container = Dxbc::parse(bytes)?;
    let part = |tag: &[u8; 4]| {
        container
            .chunk(tag)
            .map(|c| &bytes[c.offset..c.offset + c.size])
    };
    let signature = |tags: &[&[u8; 4]]| -> Result<Option<Vec<SigElement>>, String> {
        tags.iter()
            .find_map(|tag| part(tag).map(|payload| parse_signature_part(tag, payload)))
            .transpose()
    };
    let validation = part(b"PSV0");
    let threads = validation.and_then(psv::parse_compute_threads);
    let code = match (part(b"DXIL"), part(b"ACCC")) {
        (Some(payload), _) => native_dxil(payload)?,
        (_, Some(payload)) => {
            native_compute::extract(payload, threads.ok_or("missing compute group dimensions")?)?
        }
        _ => return Err("DXBC contains no native Xbox program".into()),
    };
    let ps09_pixel_inputs = match part(b"TBGP") {
        Some(config) if code.stage == Stage::Pixel && config.len() == 380 => {
            Some(u32::from_le_bytes(config[88..92].try_into().unwrap()))
        }
        Some(_) => return Err("invalid PS09 pixel configuration".into()),
        None => None,
    };
    let inputs = signature(&[b"ISG1", b"ISGN"])?;
    let outputs = signature(&[b"OSG1", b"OSG5", b"OSGN"])?;
    let primitive_sig = if code.stage == Stage::Mesh {
        signature(&[b"PSG1"])?.unwrap_or_default()
    } else {
        Vec::new()
    };
    let root = part(b"RTS0");
    Ok(ShaderExtract {
        stage: code.stage,
        shader_model: code.sm,
        required_features: part(b"SFI0")
            .and_then(|p| p.try_into().ok())
            .map(u64::from_le_bytes),
        entry: code.entry,
        isa: code.isa,
        isa_data: code.isa_data,
        isa_offset_in_fssm: code.isa_off,
        isa_preamble: code.isa_preamble,
        fssm: code.fssm,
        bitcode_stripped: code.bitcode_stripped,
        fspx_config: code.fspx_config,
        ps09_pixel_inputs,
        psv_threads: threads,
        has_input_sig: inputs.is_some(),
        has_output_sig: outputs.is_some(),
        input_sig: inputs.unwrap_or_default(),
        output_sig: outputs.unwrap_or_default(),
        primitive_sig,
        has_psv0: validation.is_some(),
        resources: validation
            .map(psv::parse_psv0)
            .transpose()?
            .unwrap_or_default(),
        gs_info: validation.and_then(psv::parse_psv0_gs),
        mesh_info: validation.and_then(psv::parse_mesh_info),
        has_root_sig: root.is_some(),
        root_sig: root.map(rootsig::parse_rts0).transpose()?,
    })
}
