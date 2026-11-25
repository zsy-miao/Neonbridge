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
//!
//! The halves meet through bounded arrival counters by default (xcl 066ae5b,
//! `x64_spin` / PRELUDE_X64 `xcl_x_meet`): a group barrier the two halves
//! reach apart hangs the GPU, while a spin that waits 2^20 polls gives up,
//! and the wave finishes on its own half's values. Slots alternate by the
//! parity of the exchange, since the other half is at most one exchange
//! ahead. TB_WAVE64_SPIN=0 meets at a group barrier instead.

/// Groupshared words the exchange declares.
pub(super) const WORDS: u32 = 1 + 2 + 4 + 128;

fn spin() -> bool {
    std::env::var("TB_WAVE64_SPIN").map_or(true, |v| v.trim() != "0")
}

/// The split form's declarations and helpers. They come before every other
/// line of the program, so the macros at the end send the wave intrinsics in
/// the rest of it (the emitter's helpers included) through the exchange.
pub(super) fn prelude([tx, ty, tz]: [u32; 3]) -> String {
    let threads = tx * ty * tz;
    let spin = u32::from(spin());
    format!(r#"#ifdef TB_NO_WAVESIZE_64
// Split wave64 (xcl 3ecf4cf / 77a5eee / e40ac38, GPL-2.0): this console wave
// runs as two 32-lane halves of one thread group.
groupshared uint tb_x_next;
groupshared uint tb_x_c[2];
groupshared uint tb_x_w[4];
groupshared uint tb_x_v[128];
static uint tb_x_half = 0u;
// Exchanges so far (the same in both halves), the other half's lanes, and
// whether the other half failed to arrive in time.
static uint tb_x_k = 0u;
static uint tb_x_other = 0u;
static bool tb_x_dead = false;
// Hardware waves need not hold consecutive thread indices: each takes the
// next 32 console threads as it arrives.
uint3 tb_x_start(uint3 hw) {{
    if (hw.x + {tx}u * (hw.y + {ty}u * hw.z) == 0u) {{ tb_x_next = 0u; tb_x_c[0] = 0u; tb_x_c[1] = 0u; }}
    GroupMemoryBarrierWithGroupSync();
    tb_x_other = {threads}u - WaveActiveCountBits(true);
    uint base = 0u;
    if (WaveIsFirstLane()) InterlockedAdd(tb_x_next, 1u, base);
    tb_x_half = WaveReadLaneFirst(base) & 1u;
    uint t = tb_x_half * 32u + WaveGetLaneIndex();
    return uint3(t % {tx}u, (t / {tx}u) % {ty}u, t / ({tx}u * {ty}u));
}}
uint tb_x_lane() {{ return tb_x_half * 32u + WaveGetLaneIndex(); }}
// Meet the other half after writing this exchange's slot: true when its
// words are readable (xcl 066ae5b xcl_x_meet).
bool tb_x_meet() {{
    uint k = tb_x_k;
    tb_x_k = k + 1u;
#if {spin}
    if (tb_x_dead) return false;
    GroupMemoryBarrier();
    uint lanes = WaveActiveCountBits(true);
    uint ok = 1u;
    if (WaveIsFirstLane()) {{
        uint o;
        InterlockedAdd(tb_x_c[tb_x_half], lanes, o);
        uint need = (k + 1u) * tb_x_other;
        [loop] for (uint n = 0u; ; n++) {{
            uint c;
            InterlockedAdd(tb_x_c[tb_x_half ^ 1u], 0u, c);
            if ((int)(c - need) >= 0) break;
            if (n >= (1u << 20)) {{ ok = 0u; break; }}
        }}
    }}
    if (WaveReadLaneFirst(ok) == 0u) {{ tb_x_dead = true; return false; }}
    GroupMemoryBarrier();
#else
    GroupMemoryBarrierWithGroupSync();
#endif
    return true;
}}
// One word per half: each half gives its value, both read both.
uint2 tb_x_pair(uint mine) {{
    uint s = (tb_x_k & 1u) * 2u;
    tb_x_w[s + tb_x_half] = mine;
    uint o = tb_x_meet() ? tb_x_w[s + (tb_x_half ^ 1u)] : 0u;
    return tb_x_half != 0u ? uint2(o, mine) : uint2(mine, o);
}}
uint4 tb_x_ballot(bool b) {{ return uint4(tb_x_pair(WaveActiveBallot(b).x), 0u, 0u); }}
bool tb_x_any(bool b) {{ uint4 m = tb_x_ballot(b); return (m.x | m.y) != 0u; }}
bool tb_x_all(bool b) {{ uint4 m = tb_x_ballot(b); uint4 a = tb_x_ballot(true); return m.x == a.x && m.y == a.y; }}
uint tb_x_count(bool b) {{ uint4 m = tb_x_ballot(b); return countbits(m.x) + countbits(m.y); }}
// Any lane, uniform or not: every lane gives its value.
uint tb_x_read(uint v, uint l) {{
    uint s = (tb_x_k & 1u) * 64u;
    tb_x_v[s + tb_x_lane()] = v;
    bool met = tb_x_meet();
    uint r = tb_x_v[s + (l & 63u)];
    return met || ((l >> 5) & 1u) == tb_x_half ? r : 0u;
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
