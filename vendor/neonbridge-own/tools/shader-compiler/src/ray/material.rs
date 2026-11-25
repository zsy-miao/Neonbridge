//! Native material hit programs run once for the committed triangle. The shader record
//! supplies geometry addresses, material bindings and instance constants.
use super::{continuation, fingerprint};
use crate::rdna2::{Opnd, instructions};
use tuxbox_dxbc::ray::State;
mod any_hit;
mod candidate;

// These programs share the local geometry ABI.
// The export string can vary independently of the shader machine code.
pub(super) fn payload_words(state: &State<'_>) -> Option<usize> {
    if state.kind != 2 || state.hit_group.is_none() {
        return None;
    }
    if state.program.len() == 2920 && fingerprint(state.program, &[]) == 0xe2c0c21503dcc5d4 {
        return Some(5);
    }
    let span = continuation::leaf_span(state).ok()?;
    // The full triangle-material ABI reads flags from payload[9], writes hit
    // distance to payload[0], and packs a sixteen-word result. Material graphs
    // can change every resource and instruction between these accesses.
    let mut stores = 0u16;
    let mut flags = false;
    let mut hit_distance = false;
    let mut entry = true;
    let mut distance_registers = vec![6];
    for op in instructions(&state.program[span]) {
        entry &= continuation::target(&op).is_none();
        if entry && !hit_distance {
            if continuation::scratch(&op)
                && op.aux.mask == 1
                && op.aux.imm == 0
                && op.mnemonic == "buffer_store_dword"
            {
                hit_distance =
                    matches!(op.dst.as_slice(), [Opnd::Vgpr(r)] if distance_registers.contains(r));
            }
            let copied = op.mnemonic == "v_mov_b32"
                && matches!(op.src.as_slice(), [Opnd::Vgpr(r)] if distance_registers.contains(r));
            distance_registers.retain(|&r| !continuation::writes(&op, r));
            if copied {
                if let [Opnd::Vgpr(reg)] = op.dst.as_slice() {
                    distance_registers.push(*reg);
                }
            }
        }

        if !continuation::scratch(&op) || op.aux.mask != 1 {
            continue;
        }
        let offset = op.aux.imm;
        if op.mnemonic == "buffer_load_dword" && offset == 36 {
            flags = true;
        } else if op.mnemonic == "buffer_store_dword"
            && (0..64).contains(&offset)
            && offset % 4 == 0
        {
            stores |= 1 << (offset / 4);
        }
    }
    (flags && hit_distance && stores == u16::MAX).then_some(16)
}

pub(super) fn translated(state: &State<'_>, words: usize) -> Result<String, String> {
    // The Lumen geometry hit writes distance, geometry user data, normal and
    // two instance indices. The full material programs have eleven more words.
    let vgprs = super::vgpr_count(state);
    let mut leaf = continuation::leaf_body(state, words)?;
    let candidate = any_hit::export(state, &mut leaf.declarations, words)?;
    // v2 and v3 identify the instance and triangle nodes. The memory lowerer
    // reconstructs their fields from DXR intrinsics. The encoded scene base
    // in v16 and the local record base in v5 are private to that lowering.
    Ok(format!(
        r#"{declarations}
struct MaterialPayload {{ uint words[{words}]; }};
[shader("closesthit")]
void {name}(inout MaterialPayload payload, BuiltInTriangleIntersectionAttributes attributes) {{
    uint s[128] = (uint[128])0, v[{vgprs}] = (uint[{vgprs}])0;
    uint exec = ~0u, scc = 1u, vcc = 0u, m0 = 0u;
    uint spill[{frame}] = (uint[{frame}])0;
    [unroll] for (uint i = 0; i < {words}; i++) spill[i] = payload.words[i];
    v[2] = 8u; v[3] = 8u; v[4] = HitKind(); v[6] = asuint(RayTCurrent());
    v[7] = {words}u; v[14] = asuint(attributes.barycentrics.x); v[15] = asuint(attributes.barycentrics.y);
    v[16] = 0x00300000u;
    uint3 origin = asuint(WorldRayOrigin()), direction = asuint(WorldRayDirection());
    v[19] = origin.x; v[20] = origin.y; v[21] = origin.z; v[22] = asuint(RayTMin());
    v[23] = direction.x; v[24] = direction.y; v[25] = direction.z;
    {body}
    [unroll] for (uint i = 0; i < {words}; i++) payload.words[i] = spill[i];
}}
{candidate}"#,
        frame = words + 1,
        name = state.name,
        declarations = leaf.declarations,
        body = leaf.body
    ))
}
