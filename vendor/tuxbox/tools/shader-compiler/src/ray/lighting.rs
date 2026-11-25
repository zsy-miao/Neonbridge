//! The surface-lighting miss export consumes a packed material payload. It
//! reads the light's eight uint4 records, area-light textures and view constants
//! from the descriptor tables in the selected local shader record.
use super::{continuation, fingerprint, resources};
use crate::rdna2::{Enc, Insn, Opnd, instructions};
use std::{collections::BTreeMap, fmt::Write, ops::Range};
use tuxbox_dxbc::ray::State;

pub(super) fn matches(state: &State<'_>) -> bool {
    state.kind == 4
        && state.program.len() == 95336
        && fingerprint(state.program, &[]) == 0xecea6123650fae56
}

const DECLARATIONS: &str = r#"
Texture2D<float4> lightTexture[13] : register(t0, space0);
StructuredBuffer<uint4> lightData : register(t13, space0);
SamplerState lightSampler[5] : register(s0, space0);
cbuffer LightingView : register(b0, space0) { uint4 lightView[245]; };
cbuffer LightingParameters : register(b1, space0) { uint4 lightParameters; };
"#;

pub(super) fn translated(state: &State<'_>) -> Result<String, String> {
    let vgprs = super::vgpr_count(state);
    let leaf = continuation::leaf_body(state, 16)?;
    Ok(format!(
        r#"{}
struct LightingPayload {{ uint words[16]; }};
[shader("miss")]
void {}(inout LightingPayload payload) {{
    uint s[128] = (uint[128])0, v[{vgprs}] = (uint[{vgprs}])0;
    uint exec = ~0u, scc = 1u, vcc = 0u, m0 = 0u;
    uint spill[17] = (uint[17])0;
    [unroll] for (uint i=0; i<16; ++i) spill[i] = payload.words[i];
    v[6] = asuint(RayTCurrent()); v[7] = 16u; v[17] = RayFlags();
    uint3 origin = asuint(WorldRayOrigin()), direction = asuint(WorldRayDirection());
    v[19] = origin.x; v[20] = origin.y; v[21] = origin.z; v[22] = asuint(RayTMin());
    v[23] = direction.x; v[24] = direction.y; v[25] = direction.z;
    {}
    payload.words[3] = spill[3]; payload.words[4] = spill[4];
}}
"#,
        leaf.declarations, state.name, leaf.body
    ))
}

pub(super) fn bindings(
    state: &State<'_>,
    span: &Range<usize>,
) -> Result<(String, BTreeMap<usize, String>), String> {
    let mut code: Vec<_> = instructions(&state.program[span.clone()]).collect();
    for ins in &mut code {
        ins.offset += span.start;
    }
    let mut emitted = BTreeMap::new();
    for (i, ins) in code.iter().enumerate() {
        if !matches!(ins.enc, Enc::Smem | Enc::Flat | Enc::Mimg | Enc::Mubuf)
            || continuation::scratch(ins)
        {
            continue;
        }
        let next = code[i + 1..]
            .iter()
            .find(|op| !matches!(op.mnemonic.as_str(), "s_nop" | "s_waitcnt"));
        let body = memory(ins, next).ok_or_else(|| {
            format!(
                "unsupported lighting resource {} at {:#x}",
                ins.mnemonic, ins.offset
            )
        })?;
        emitted.insert(ins.offset, body);
    }
    Ok((resources::local_record(&state.name) + DECLARATIONS, emitted))
}

fn memory(ins: &Insn, next: Option<&Insn>) -> Option<String> {
    let &[dst] = ins.dst.as_slice() else {
        return None;
    };
    match (ins.enc, dst, ins.src.as_slice()) {
        // FLAT reads one of the three table handles at record bytes 64/72/80.
        // DXR selects the record for this invocation. The guest only adds byte
        // offsets to those handles before loading a descriptor from one table.
        (Enc::Flat, Opnd::Vgpr(d), [Opnd::Vgpr(_), _, Opnd::Sgpr(_)])
            if ins.op == 13 && ins.aux.imm == 0 =>
        {
            Some(format!("v[{d}] = 0u; v[{}] = 0u;", d + 1))
        }
        (Enc::Smem, Opnd::Sgpr(d), [Opnd::Sgpr(base)])
            if ins.aux.imm == 0 && ins.raw[1] >> 25 == 125 =>
        {
            let op = next?.mnemonic.as_str();
            let body = match (ins.op, op) {
                (3, "v_lshrrev_b32" | "v_lshl_or_b32" | "s_bfe_u32") => {
                    // The four retail extracts reconstruct (width-1), (height-1)
                    // and BASE_LEVEL from the native image descriptor. Desktop
                    // GetDimensions(0) is already relative to the view base.
                    format!(
                        "{{ uint w,h; lightTexture[NonUniformResourceIndex(s[{base}] >> 5u)].GetDimensions(w,h); --w; --h; s[{}]=w << 30u; s[{}]=(w >> 2u) | (h << 14u); s[{}]=0u; }}",
                        d + 1,
                        d + 2,
                        d + 3
                    )
                }
                (3, "s_mov_b32" | "image_load") => format!("s[{d}] = s[{base}] >> 5u;"),
                (2, "image_sample_l" | "image_sample_lz") => format!("s[{d}] = s[{base}] >> 4u;"),
                (2, op) if op.starts_with("buffer_load_") => String::new(),
                _ => return None,
            };
            Some(body)
        }
        (Enc::Mubuf, Opnd::Vgpr(d), [Opnd::Vgpr(at), Opnd::Sgpr(_), Opnd::ConstInt(0)]) => {
            let count = match ins.op {
                12 => 1,
                13 => 2,
                14 => 4,
                15 => 3,
                _ => return None,
            };
            let word = usize::try_from(ins.aux.imm).ok()? / 4;
            if count == 1 && word == 1 {
                return match ins.aux.mask {
                    0 => Some(format!("v[{d}]=lightParameters.y;")),
                    2 => Some(format!("v[{d}]=lightView[v[{at}]].y;")),
                    _ => None,
                };
            }
            if ins.aux.mask != 2 || word + count > 4 {
                return None;
            }
            let mut body = format!("{{ uint4 data=lightData[v[{at}]];");
            for lane in 0..count {
                write!(body, " v[{}]=data[{}];", usize::from(d) + lane, word + lane).ok()?;
            }
            body.push_str(" }");
            Some(body)
        }
        (Enc::Mimg, Opnd::Vgpr(d), src) if ins.aux.imm == 1 && ins.raw[1] & (1 << 30) == 0 => {
            let [Opnd::Sgpr(tex), Opnd::Sgpr(samp)] = src.get(src.len().checked_sub(2)?..)? else {
                return None;
            };
            let coords = resources::coords(ins, 2 + usize::from(ins.op == 36))?;
            let uv = format!("uint2(v[{}],v[{}])", coords[0], coords[1]);
            let texture = format!("lightTexture[NonUniformResourceIndex(s[{tex}])]");
            let sampler = format!("lightSampler[NonUniformResourceIndex(s[{samp}])]");
            let sample = match ins.op {
                0 => format!("{texture}.Load(int3({uv},0))"),
                36 => format!(
                    "{texture}.SampleLevel({sampler},asfloat({uv}),asfloat(v[{}]))",
                    coords[2]
                ),
                39 => format!("{texture}.SampleLevel({sampler},asfloat({uv}),0.0f)"),
                _ => return None,
            };
            resources::save_components(d.into(), ins.aux.mask, &format!("asuint({sample})"))
        }
        _ => None,
    }
}
