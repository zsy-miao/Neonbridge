//! Wave64 compute programs on GPUs with 32-lane waves only: each console wave
//! runs as the two wave32 halves of its thread group, which meet through
//! groupshared memory wherever a value has to be whole-wave (ballots, lane
//! reads, reductions, prefixes). Per-lane code is unchanged.
//!
//! Ported from xcl 3ecf4cf, 77a5eee and e40ac38 and docs/wave64-on-wave32.md
//! (ivall and contributors, GPL-2.0, copied with the user's approval). Only
//! the one-console-wave form is ported. A group holds 33..64 threads, so both
//! hardware waves of the group are the two halves of the same console wave.
//! They run the same sequence of exchanges, which makes a group barrier
//! between writing and reading the slot legal. Not ported: groups of more than
//! one console wave (xcl's counter-paired variant) and xcl's liveness pass for
//! lane-local masks. Every mask is exchanged.
//!
//! The form is emitted under `#ifdef TB_NO_WAVESIZE_64`, which the runtime
//! defines where the device's wave range excludes 64. Elsewhere the HLSL is
//! the same as before. TB_SPLIT_WAVE64=0 at translation time turns it off.

/// Groupshared words the exchange declares.
pub(super) const WORDS: u32 = 67;

/// The split form's declarations and helpers. They come before every other
/// line of the program, so the macros at the end send the wave intrinsics in
/// the rest of it (the emitter's helpers included) through the exchange.
pub(super) fn prelude([tx, ty, _]: [u32; 3]) -> String {
    format!(r#"#ifdef TB_NO_WAVESIZE_64
// Split wave64 (xcl 3ecf4cf / 77a5eee / e40ac38, GPL-2.0): this console wave
// runs as two 32-lane halves of one thread group.
groupshared uint tb_x_next;
groupshared uint tb_x_w[2];
groupshared uint tb_x_v[64];
static uint tb_x_half = 0u;
// Hardware waves need not hold consecutive thread indices: each takes the
// next 32 console threads as it arrives.
uint3 tb_x_start(uint3 hw) {{
    if (hw.x + {tx}u * (hw.y + {ty}u * hw.z) == 0u) tb_x_next = 0u;
    GroupMemoryBarrierWithGroupSync();
    uint base = 0u;
    if (WaveIsFirstLane()) InterlockedAdd(tb_x_next, 1u, base);
    tb_x_half = WaveReadLaneFirst(base) & 1u;
    uint t = tb_x_half * 32u + WaveGetLaneIndex();
    return uint3(t % {tx}u, (t / {tx}u) % {ty}u, t / ({tx}u * {ty}u));
}}
uint tb_x_lane() {{ return tb_x_half * 32u + WaveGetLaneIndex(); }}
// One word per half: each half gives its value, both read both.
uint2 tb_x_pair(uint mine) {{
    tb_x_w[tb_x_half] = mine;
    GroupMemoryBarrierWithGroupSync();
    uint2 r = uint2(tb_x_w[0], tb_x_w[1]);
    GroupMemoryBarrierWithGroupSync();
    return r;
}}
uint4 tb_x_ballot(bool b) {{ return uint4(tb_x_pair(WaveActiveBallot(b).x), 0u, 0u); }}
bool tb_x_any(bool b) {{ uint4 m = tb_x_ballot(b); return (m.x | m.y) != 0u; }}
bool tb_x_all(bool b) {{ uint4 m = tb_x_ballot(b); uint4 a = tb_x_ballot(true); return m.x == a.x && m.y == a.y; }}
uint tb_x_count(bool b) {{ uint4 m = tb_x_ballot(b); return countbits(m.x) + countbits(m.y); }}
// Any lane, uniform or not: every lane gives its value.
uint tb_x_read(uint v, uint l) {{
    tb_x_v[tb_x_lane()] = v;
    GroupMemoryBarrierWithGroupSync();
    uint r = tb_x_v[l & 63u];
    GroupMemoryBarrierWithGroupSync();
    return r;
}}
int tb_x_read(int v, uint l) {{ return asint(tb_x_read(asuint(v), l)); }}
float tb_x_read(float v, uint l) {{ return asfloat(tb_x_read(asuint(v), l)); }}
bool tb_x_read(bool v, uint l) {{ return tb_x_read(v ? 1u : 0u, l) != 0u; }}
uint tb_x_first_lane() {{ uint4 m = tb_x_ballot(true); return m.x != 0u ? firstbitlow(m.x) : 32u + firstbitlow(m.y); }}
uint tb_x_first(uint v) {{ return tb_x_read(v, tb_x_first_lane()); }}
int tb_x_first(int v) {{ return asint(tb_x_first(asuint(v))); }}
float tb_x_first(float v) {{ return asfloat(tb_x_first(asuint(v))); }}
bool tb_x_is_first() {{ return tb_x_lane() == tb_x_first_lane(); }}
uint tb_x_sum(uint v) {{ uint2 p = tb_x_pair(WaveActiveSum(v)); return p.x + p.y; }}
int tb_x_sum(int v) {{ uint2 p = tb_x_pair(asuint(WaveActiveSum(v))); return asint(p.x) + asint(p.y); }}
float tb_x_sum(float v) {{ uint2 p = tb_x_pair(asuint(WaveActiveSum(v))); return asfloat(p.x) + asfloat(p.y); }}
uint tb_x_min(uint v) {{ uint2 p = tb_x_pair(WaveActiveMin(v)); return min(p.x, p.y); }}
int tb_x_min(int v) {{ uint2 p = tb_x_pair(asuint(WaveActiveMin(v))); return min(asint(p.x), asint(p.y)); }}
float tb_x_min(float v) {{ uint2 p = tb_x_pair(asuint(WaveActiveMin(v))); return min(asfloat(p.x), asfloat(p.y)); }}
uint tb_x_max(uint v) {{ uint2 p = tb_x_pair(WaveActiveMax(v)); return max(p.x, p.y); }}
int tb_x_max(int v) {{ uint2 p = tb_x_pair(asuint(WaveActiveMax(v))); return max(asint(p.x), asint(p.y)); }}
float tb_x_max(float v) {{ uint2 p = tb_x_pair(asuint(WaveActiveMax(v))); return max(asfloat(p.x), asfloat(p.y)); }}
uint tb_x_or(uint v) {{ uint2 p = tb_x_pair(WaveActiveBitOr(v)); return p.x | p.y; }}
uint tb_x_and(uint v) {{ uint2 p = tb_x_pair(WaveActiveBitAnd(v)); return p.x & p.y; }}
uint tb_x_xor(uint v) {{ uint2 p = tb_x_pair(WaveActiveBitXor(v)); return p.x ^ p.y; }}
// Prefixes: the upper half adds the lower half's total.
uint tb_x_prefix_sum(uint v) {{ uint2 p = tb_x_pair(WaveActiveSum(v)); return WavePrefixSum(v) + (tb_x_half != 0u ? p.x : 0u); }}
int tb_x_prefix_sum(int v) {{ uint2 p = tb_x_pair(asuint(WaveActiveSum(v))); return WavePrefixSum(v) + (tb_x_half != 0u ? asint(p.x) : 0); }}
float tb_x_prefix_sum(float v) {{ uint2 p = tb_x_pair(asuint(WaveActiveSum(v))); return WavePrefixSum(v) + (tb_x_half != 0u ? asfloat(p.x) : 0.0f); }}
uint tb_x_prefix_count(bool b) {{ uint2 p = tb_x_pair(WaveActiveCountBits(b)); return WavePrefixCountBits(b) + (tb_x_half != 0u ? p.x : 0u); }}
#define WaveActiveBallot(b) tb_x_ballot(b)
#define WaveActiveAnyTrue(b) tb_x_any(b)
#define WaveActiveAllTrue(b) tb_x_all(b)
#define WaveActiveCountBits(b) tb_x_count(b)
#define WaveReadLaneAt(v, l) tb_x_read(v, l)
#define WaveReadLaneFirst(v) tb_x_first(v)
#define WaveIsFirstLane() tb_x_is_first()
#define WaveActiveSum(v) tb_x_sum(v)
#define WaveActiveMin(v) tb_x_min(v)
#define WaveActiveMax(v) tb_x_max(v)
#define WaveActiveBitOr(v) tb_x_or(v)
#define WaveActiveBitAnd(v) tb_x_and(v)
#define WaveActiveBitXor(v) tb_x_xor(v)
#define WavePrefixSum(v) tb_x_prefix_sum(v)
#define WavePrefixCountBits(b) tb_x_prefix_count(b)
#define WaveGetLaneIndex() tb_x_lane()
#define WaveGetLaneCount() 64u
#endif
"#)
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_prelude_reroutes_every_wave_intrinsic_after_its_helpers() {
        let p = super::prelude([8, 8, 1]);
        assert!(p.starts_with("#ifdef TB_NO_WAVESIZE_64"));
        let defines = p.find("#define WaveActiveBallot").unwrap();
        assert!(p[..defines].contains("WaveActiveBallot(b).x"), "helpers use the hardware intrinsic");
        assert!(p.contains("uint3(t % 8u, (t / 8u) % 8u, t / (8u * 8u))"));
    }
}
