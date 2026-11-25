//! Compact the free pages in a virtual shadow map. Native MBCNT and FF1
//! elect one allocator per logical wave64, independently of host wave size.
use crate::ShaderExtract;

pub(super) fn pack_free_pages(shader: &ShaderExtract) -> Option<String> {
    if shader.entry.as_deref() != Some("PackFreePages")
        || shader.psv_threads != Some([256, 1, 1])
        || shader.isa.len() != 280
        || shader.isa.iter().fold(0xcbf29ce484222325u64, |h, &v| {
            (h ^ u64::from(v)).wrapping_mul(0x100000001b3)
        }) != 0xf740ad1f82b432e5
        || !shader
            .resources
            .iter()
            .map(|r| (r.res_type, r.space, r.lower, r.upper))
            .eq([(2, 0, 0, 0), (5, 0, 0, 0), (8, 0, 0, 0)])
    {
        return None;
    }
    let image_size =
        usize::try_from(u64::from_le_bytes(*shader.fssm.get(8..)?.first_chunk()?)).ok()?;
    let image = shader.fssm.get(16..)?.get(..image_size)?;
    let binding_words = super::series_resource_bindings(image)?.as_chunks::<4>().0;
    const BINDINGS: [u32; 15] = [
        0x300, 0, 0, 8, 0x404, 0x302, 0, 0, 0, 0x10504, 0x303, 0, 0, 4, 0x30704,
    ];
    if !binding_words
        .iter()
        .map(|&word| u32::from_le_bytes(word))
        .eq(BINDINGS)
    {
        return None;
    }
    // FSSM root tables s4, s5 and s7 supply t0, b0 and u0. FSSM
    // descriptors give the physical page metadata an 8-byte stride and
    // the packed list a 4-byte stride. Native code reads metadata word 0.
    Some(
        [
            r#"cbuffer PageParameters : register(b0) { uint4 pageParameters[9]; };
StructuredBuffer<uint2> pageMetadata : register(t0);
RWStructuredBuffer<uint> tb_page_list : register(u0);
"#,
            RESERVE,
            r#"
[numthreads(256,1,1)]
void main(uint3 tid : SV_DispatchThreadID, uint groupIndex : SV_GroupIndex) {
    uint pageCount = pageParameters[8].w;
    bool free = false;
    if (tid.x < pageCount) free = pageMetadata[tid.x].x == 0u;
    uint index = tb_reserve64(groupIndex, free, pageCount);
    if (free) tb_page_list[index] = tid.x;
}
"#,
        ]
        .concat(),
    )
}

// Four guest waves share a group. Wave leaders reserve independently so a
// nonempty wave's output is contiguous in increasing native lane order.
pub(crate) const RESERVE: &str = r#"
groupshared uint tb_page_masks[8], tb_page_bases[4];
uint tb_reserve64(uint groupIndex, bool active, uint counterIndex) {
    uint wave = groupIndex >> 6, lane = groupIndex & 63u;
    uint halfWave = groupIndex >> 5, bit = 1u << (lane & 31u);
    if (groupIndex < 8u) tb_page_masks[groupIndex] = 0u;
    GroupMemoryBarrierWithGroupSync();
    if (active) InterlockedOr(tb_page_masks[halfWave], bit);
    GroupMemoryBarrierWithGroupSync();
    uint maskLo = tb_page_masks[wave * 2u], maskHi = tb_page_masks[wave * 2u + 1u];
    if (lane == 0u) {
        uint count = countbits(maskLo) + countbits(maskHi), base = 0u;
        if (count != 0u) InterlockedAdd(tb_page_list[counterIndex], count, base);
        tb_page_bases[wave] = base;
    }
    GroupMemoryBarrierWithGroupSync();
    uint prefix = lane < 32u ? countbits(maskLo & (bit - 1u)) :
                               countbits(maskLo) + countbits(maskHi & (bit - 1u));
    return tb_page_bases[wave] + prefix;
}
"#;
