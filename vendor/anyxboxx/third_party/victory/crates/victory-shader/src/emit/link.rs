//! A mesh program and the pixel program drawn with it, translated for one host pipeline
//! ([`link_mesh`]).
//!
//! D3D12 caps a mesh shader's outputs at 32 KiB: 4 bytes per component of every vertex
//! element times the declared vertex maximum rounded up to 32, plus the same for primitive
//! elements (`container::mesh_output_bytes`, as the validator measures it). 71 of the
//! title's mesh programs pass it as the console declares their outputs: pedestrian,
//! vehicle and prop transforms with fire, spatter, decal and transmission variants, 128
//! vertices of 65 to 71 components (33,280 to 36,352 bytes). NVIDIA's driver builds their
//! pipelines anyway, but the programs are invalid. D3D12 links the two stages by semantic,
//! and a pipeline needs only the outputs its pixel program reads: of the title's pixel
//! programs whose inputs such a mesh program writes, the one that reads most reads 55
//! components (a pedestrian fire prepass). So the pair is translated again for its
//! pipeline: the mesh program declares and writes only the outputs the pixel program reads
//! (and its system values), and the pixel program leaves out the inputs it never reads,
//! which D3D12 would otherwise refuse as inputs the mesh program does not write. The
//! elements kept keep their rows and columns, each program its code and bindings. Measured
//! offline (`victory translate` with `VICTORY_PAIRS`): the ten pixel programs that read most
//! for each of the 71, and the three pairs drawn in the runs so far, all link, validate and
//! get a pipeline on an RTX 5090.

use super::container::{MESH_OUTPUT_LIMIT, SigElement, mesh_output_bytes};
use super::{HostShader, translate};
use crate::dxbc::{Container, FourCC, u32_at};
use crate::native::HwStage;
use crate::ShaderError;

thread_local! {
    /// See [`with_linked`].
    static LINKED: std::cell::RefCell<Option<Linked>> = const { std::cell::RefCell::new(None) };
}

/// What a stage leaves out of its signatures for its partner in one host pipeline
/// ([`link_mesh`]): elements of arbitrary semantics, by name and index.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct Linked {
    pub inputs: Vec<(String, u32)>,
    pub outputs: Vec<(String, u32)>,
}

/// Runs `f` with programs lowered for a linked pipeline: the elements `linked` names leave
/// the signatures; stores to such an output are not made, and the other elements keep their
/// rows and modes (ids count only what is kept). An input left out must be one the program
/// never reads: a lowering that reads one fails.
pub(super) fn with_linked<R>(linked: Linked, f: impl FnOnce() -> R) -> R {
    let was = LINKED.replace(Some(linked));
    let r = f();
    LINKED.set(was);
    r
}

/// Per element of `inputs` and of `outputs` (the signatures as the console declares them),
/// whether the stage being lowered leaves it out ([`with_linked`]); empty when not linking.
pub(super) fn left_out(inputs: &[SigElement], outputs: &[SigElement]) -> (Vec<bool>, Vec<bool>) {
    LINKED.with_borrow(|l| match l {
        Some(l) => {
            let of = |els: &[SigElement], names: &[(String, u32)]| {
                els.iter()
                    .map(|e| e.kind == 0 && names.iter().any(|(n, i)| n.eq_ignore_ascii_case(&e.name) && *i == e.index))
                    .collect::<Vec<bool>>()
            };
            (of(inputs, &l.inputs), of(outputs, &l.outputs))
        }
        None => (Vec::new(), Vec::new()),
    })
}

/// The translations of the mesh program `ms` (container `ms_container`) and the pixel program
/// `ps` (container `ps_container`) for one host pipeline, when `ms`'s outputs pass D3D12's 32
/// KiB as translated alone: the mesh program without the outputs the pixel program does not
/// read, the pixel program without the inputs it does not read (see the module notes).
///
/// `None` (make the pipeline of `ms` and `ps` as they are) when `ms` fits, when the pixel
/// program reads every output `ms` has, when the linked mesh program still does not fit, or
/// when a linked translation binds anything differently from its shared one (the pipeline's
/// root signature is made from the shared ones).
pub fn link_mesh(
    ms_container: &[u8],
    ms: &HostShader,
    ps_container: &[u8],
    ps: &HostShader,
) -> Result<Option<(HostShader, HostShader)>, ShaderError> {
    if ms.stage != HwStage::Ms || ps.stage != HwStage::Ps || mesh_fits(&ms.dxil)? {
        return Ok(None);
    }
    let named = |e: &Element| (e.name.clone(), e.index);
    let inputs = elements(&ps.dxil, FourCC::ISG1)?;
    let read = |e: &Element| {
        inputs
            .iter()
            .any(|i| i.arbitrary() && i.used != 0 && i.name.eq_ignore_ascii_case(&e.name) && i.index == e.index)
    };
    let outputs: Vec<(String, u32)> = elements(&ms.dxil, FourCC::OSG1)?
        .iter()
        .filter(|e| e.arbitrary() && !read(e))
        .map(named)
        .collect();
    if outputs.is_empty() {
        return Ok(None);
    }
    let unread: Vec<(String, u32)> = inputs.iter().filter(|i| i.arbitrary() && i.used == 0).map(named).collect();
    let linked_ms = with_linked(
        Linked {
            inputs: Vec::new(),
            outputs,
        },
        || translate(ms_container),
    )?;
    let linked_ps = if unread.is_empty() {
        ps.clone()
    } else {
        with_linked(
            Linked {
                inputs: unread,
                outputs: Vec::new(),
            },
            || translate(ps_container),
        )?
    };
    let same = |a: &HostShader, b: &HostShader| HostShader { dxil: Vec::new(), ..a.clone() } == HostShader { dxil: Vec::new(), ..b.clone() };
    if !mesh_fits(&linked_ms.dxil)? || !same(ms, &linked_ms) || !same(ps, &linked_ps) {
        return Ok(None);
    }
    Ok(Some((linked_ms, linked_ps)))
}

/// Whether an emitted mesh program's outputs fit D3D12's 32 KiB (a program of another stage
/// fits).
pub fn mesh_fits(dxil: &[u8]) -> Result<bool, ShaderError> {
    let c = Container::parse(dxil)?;
    let Some(psv) = c.part(FourCC::PSV0) else {
        return Ok(true);
    };
    // runtime info (its size first): shader kind at byte 24, mesh maxima at bytes 12 and 14
    let info = psv.data.get(4..).ok_or(ShaderError::Malformed("truncated"))?;
    if info.get(24) != Some(&13) {
        return Ok(true);
    }
    let half = |at: usize| info.get(at..at + 2).map_or(0, |b| u32::from(u16::from_le_bytes([b[0], b[1]])));
    let declared = |fourcc| -> Result<Vec<SigElement>, ShaderError> {
        Ok(elements(dxil, fourcc)?
            .into_iter()
            .map(|e| SigElement {
                name: e.name,
                index: e.index,
                kind: 0,
                comp: 0,
                interp: 0,
                row: None,
                col: 0,
                cols: e.mask.count_ones() as u8,
                usage: 0,
                half: false,
            })
            .collect())
    };
    let bytes = mesh_output_bytes(&declared(FourCC::OSG1)?, &declared(FourCC::PSG1)?, half(12), half(14));
    Ok(bytes <= MESH_OUTPUT_LIMIT)
}

/// A row of an emitted signature part (`container::signature`).
struct Element {
    name: String,
    index: u32,
    /// The system value (0: an arbitrary semantic).
    sv: u32,
    mask: u8,
    /// Inputs: the components read.
    used: u8,
}

impl Element {
    fn arbitrary(&self) -> bool {
        self.sv == 0
    }
}

fn elements(dxil: &[u8], fourcc: FourCC) -> Result<Vec<Element>, ShaderError> {
    let c = Container::parse(dxil)?;
    let Some(part) = c.part(fourcc) else {
        return Ok(Vec::new());
    };
    let part = part.data;
    let count = u32_at(part, 0)? as usize;
    let base = u32_at(part, 4)? as usize;
    (0..count)
        .map(|i| {
            let at = base + 32 * i;
            let name_at = u32_at(part, at + 4)? as usize;
            let name = part
                .get(name_at..)
                .and_then(|s| s.split(|&b| b == 0).next())
                .ok_or(ShaderError::Malformed("signature name out of bounds"))?;
            let bytes = part.get(at + 24..at + 26).ok_or(ShaderError::Malformed("truncated"))?;
            Ok(Element {
                name: String::from_utf8_lossy(name).into_owned(),
                index: u32_at(part, at + 8)?,
                sv: u32_at(part, at + 12)?,
                mask: bytes[0],
                used: bytes[1],
            })
        })
        .collect()
}
