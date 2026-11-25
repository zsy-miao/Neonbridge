//! Compact visualization tiles which overlap the viewport.
use crate::ShaderExtract;

pub(super) fn translate(shader: &ShaderExtract) -> Option<String> {
    if shader.entry.as_deref() != Some("FLumenVisualizeCreateTilesCS")
        || shader.psv_threads != Some([8, 8, 1]) || shader.isa.len() != 384
        || shader.isa.iter().fold(0xcbf29ce484222325u64, |h, &v| {
            (h ^ u64::from(v)).wrapping_mul(0x100000001b3)
        }) != 0x7657314caa49f280
        || !shader.resources.iter().map(|r| (r.res_type, r.space, r.lower, r.upper))
            .eq([(2, 0, 0, 0), (6, 0, 0, 0), (8, 0, 1, 1)])
    { return None; }
    Some(SOURCE.to_string())
}

const SOURCE: &str = r#"
cbuffer Parameters : register(b0) { uint4 params[135]; };
RWBuffer<uint> countOut : register(u0);
RWStructuredBuffer<uint> tilesOut : register(u1);
groupshared uint visible;
// RDNA2 V_CVT_U32_F32 saturates out-of-range values and maps NaN to zero.
uint cvt_u32(float value) {
    if (isnan(value) || value <= 0.0) return 0u;
    if (value >= 4294967296.0) return 0xffffffffu;
    return (uint)value;
}

[numthreads(8,8,1)]
void main(uint lane : SV_GroupIndex, uint3 tid : SV_DispatchThreadID, uint3 group : SV_GroupID) {
    if (lane == 0u) visible = 0u;
    GroupMemoryBarrierWithGroupSync();
    precise float x = asfloat(params[133].x) + float(tid.x);
    precise float y = asfloat(params[133].y) + float(tid.y);
    uint ux = cvt_u32(x), uy = cvt_u32(y);
    uint maxX = cvt_u32(asfloat(params[134].x));
    uint maxY = cvt_u32(asfloat(params[134].y));
    if (ux < maxX && uy < maxY) InterlockedOr(visible, 1u);
    GroupMemoryBarrierWithGroupSync();
    if (lane == 0u && visible != 0u) {
        uint index;
        InterlockedAdd(countOut[0], 1u, index);
        tilesOut[index] = (group.x & 0xffffu) | (group.y << 16u);
    }
}
"#;
