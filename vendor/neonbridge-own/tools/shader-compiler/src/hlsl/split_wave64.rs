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

/// Groupshared words the exchange declares per console wave (plus one).
pub(super) const WORDS: u32 = 1 + 2 + 4 + 128;

/// Groups of more than one console wave (xcl docs/wave64-on-wave32.md
/// "Several console waves per group", GPL-2.0, copied with credit): each
/// pair of hardware waves meets through its own counters and slots, always
/// by the bounded spin (a group barrier would wait for the other console
/// waves too). TB_SPLIT_WAVE64_MULTI=0 keeps the split to 33..64 threads.
pub(super) fn multi() -> bool {
    spin() && std::env::var("TB_SPLIT_WAVE64_MULTI").map_or(true, |v| v.trim() != "0")
}

/// Console waves in a group of `threads`.
pub(super) fn waves(threads: u32) -> u32 {
    threads.div_ceil(64)
}

fn spin() -> bool {
    std::env::var("TB_WAVE64_SPIN").map_or(true, |v| v.trim() != "0")
}

/// TB_HOST_WAVE (default on, =0 exchanges as before): a console wave64
/// program whose cross-lane code stays inside 32-lane halves (DPP rows,
/// ds_swizzle, v_permlane16/x16, which the ISA confines there) runs each host
/// wave32 as one half of a console wave with no exchange at all: the halves
/// never meet, as if the other half had already left (tb_x_dead from the
/// start), so there are no barriers or spins between them. Not when the code
/// crosses halves (v_mbcnt, v_readlane/v_writelane, ds_(b)permute, masks read
/// as numbers or built from constants, a whole-wave VCC branch) or shares
/// LDS within the console wave (see Emitter::host_wave_local). Copied from
/// ztrx victory-shader emit/lower.rs:1276-1293 (set_host_max_wave, GPL-3.0,
/// credit to its authors); TB_SPLIT_WAVE64 stays the form for the others.
pub(super) fn host_wave() -> bool {
    std::env::var("TB_HOST_WAVE").map_or(true, |v| v.trim() != "0")
}

/// The split prelude with the halves kept apart (TB_HOST_WAVE).
pub(super) fn prelude_local(group: [u32; 3]) -> String {
    let p = prelude(group);
    let p = p.replacen("static bool tb_x_dead = false;", "static bool tb_x_dead = true; // TB_HOST_WAVE: halves never meet", 1);
    p.replacen("#if 0\n", "#if 1\n", 1)
}

/// The split form's declarations and helpers. They come before every other
/// line of the program, so the macros at the end send the wave intrinsics in
/// the rest of it (the emitter's helpers included) through the exchange.
pub(super) fn prelude([tx, ty, tz]: [u32; 3]) -> String {
    let threads = tx * ty * tz;
    // 64 threads: two full hardware waves, where the one-wave form was already
    // exact. Keep its text byte for byte so those programs' HLSL (and their
    // compiled-shader cache entries) do not change.
    if threads == 64 {
        return prelude_one_wave([tx, ty, tz]);
    }
    let w = waves(threads);
    let chunks = threads.div_ceil(32);
    // Several console waves always meet by the spin.
    let spin = u32::from(spin() || w > 1);
    let (c2, w4, v128) = (2 * w, 4 * w, 128 * w);
    format!(r#"#ifdef TB_NO_WAVESIZE_64
// Split wave64 (xcl 3ecf4cf / 77a5eee / e40ac38, GPL-2.0): this console wave
// runs as two 32-lane halves of one thread group.
groupshared uint tb_x_next;
groupshared uint tb_x_c[{c2}];
groupshared uint tb_x_w[{w4}];
groupshared uint tb_x_v[{v128}];
static uint tb_x_half = 0u;
// This pair's console wave: its counters, words and lanes.
static uint tb_x_wave = 0u;
// Exchanges so far (the same in both halves), the other half's lanes, and
// whether the other half failed to arrive in time.
static uint tb_x_k = 0u;
static uint tb_x_other = 0u;
static bool tb_x_dead = false;
// Hardware waves need not hold consecutive thread indices: each takes the
// next 32 console threads as it arrives.
uint3 tb_x_start(uint3 hw) {{
    if (hw.x + {tx}u * (hw.y + {ty}u * hw.z) == 0u) {{ tb_x_next = 0u; for (uint i = 0u; i < {c2}u; i++) tb_x_c[i] = 0u; }}
    GroupMemoryBarrierWithGroupSync();
    // Full hardware waves take the next 32 console threads as they arrive; a
    // partial one (the group's last threads) takes the last chunk.
    uint base = {chunks}u - 1u;
    bool full = WaveActiveCountBits(true) == 32u;
    if (full && WaveIsFirstLane()) InterlockedAdd(tb_x_next, 1u, base);
    base = full ? WaveReadLaneFirst(base) : {chunks}u - 1u;
    tb_x_half = base & 1u;
    tb_x_wave = base >> 1u;
    // The partner chunk's lanes (0 when it does not exist: its ballot bits are 0).
    uint pb = (base ^ 1u) * 32u;
    tb_x_other = pb < {threads}u ? min({threads}u - pb, 32u) : 0u;
    uint t = base * 32u + WaveGetLaneIndex();
    return uint3(t % {tx}u, (t / {tx}u) % {ty}u, t / ({tx}u * {ty}u));
}}
uint tb_x_lane() {{ return tb_x_half * 32u + WaveGetLaneIndex(); }}
// Meet the other half after writing this exchange's slot: true when its
// words are readable (xcl 066ae5b xcl_x_meet).
bool tb_x_meet() {{
    uint k = tb_x_k;
    tb_x_k = k + 1u;
#if {spin}
    if (tb_x_dead || tb_x_other == 0u) return false;
    GroupMemoryBarrier();
    uint lanes = WaveActiveCountBits(true);
    uint ok = 1u;
    if (WaveIsFirstLane()) {{
        uint o;
        InterlockedAdd(tb_x_c[tb_x_wave * 2u + tb_x_half], lanes, o);
        uint need = (k + 1u) * tb_x_other;
        [loop] for (uint n = 0u; ; n++) {{
            uint c;
            InterlockedAdd(tb_x_c[tb_x_wave * 2u + (tb_x_half ^ 1u)], 0u, c);
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
    uint s = tb_x_wave * 4u + (tb_x_k & 1u) * 2u;
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
    uint s = tb_x_wave * 128u + (tb_x_k & 1u) * 64u;
    tb_x_v[s + tb_x_lane()] = v;
    bool met = tb_x_meet();
    uint r = tb_x_v[s + (l & 63u)];
    return met || ((l >> 5) & 1u) == tb_x_half ? r : 0u;
}}
// Lanes l and l ^ 32 of one value in one exchange (xcl 9e2c7a5).
uint2 tb_x_read2(uint v, uint l) {{
    uint s = tb_x_wave * 128u + (tb_x_k & 1u) * 64u;
    tb_x_v[s + tb_x_lane()] = v;
    bool met = tb_x_meet();
    uint a = tb_x_v[s + (l & 63u)], b = tb_x_v[s + ((l ^ 32u) & 63u)];
    bool mine = ((l >> 5) & 1u) == tb_x_half;
    return uint2(met || mine ? a : 0u, met || !mine ? b : 0u);
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

/// The one-console-wave prelude as it was before 396b1f0 (64-thread groups).
fn prelude_one_wave([tx, ty, tz]: [u32; 3]) -> String {
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
// Lanes l and l ^ 32 of one value in one exchange (xcl 9e2c7a5).
uint2 tb_x_read2(uint v, uint l) {{
    uint s = (tb_x_k & 1u) * 64u;
    tb_x_v[s + tb_x_lane()] = v;
    bool met = tb_x_meet();
    uint a = tb_x_v[s + (l & 63u)], b = tb_x_v[s + ((l ^ 32u) & 63u)];
    bool mine = ((l >> 5) & 1u) == tb_x_half;
    return uint2(met || mine ? a : 0u, met || !mine ? b : 0u);
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

/// One `sD = WaveReadLaneAt(v, ((K) & 63u & (WaveGetLaneCount() - 1u)));`
/// line: (indent, destination, value, constant lane).
fn lane_read(line: &str) -> Option<(&str, &str, &str, u32)> {
    let body = line.trim_start();
    let indent = &line[..line.len() - body.len()];
    let (dst, rest) = body.split_once(" = WaveReadLaneAt(")?;
    let (value, rest) = rest.split_once(", ((")?;
    let (k, tail) = rest.split_once(") & 63u & (WaveGetLaneCount() - 1u)));")?;
    if !tail.is_empty() || !dst.starts_with('s') || !dst[1..].bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let k = k.strip_suffix('u')?;
    let k = match k.strip_prefix("0x") { Some(h) => u32::from_str_radix(h, 16).ok()?, None => k.parse().ok()? };
    Some((indent, dst, value, k))
}

/// Fold two adjacent constant-lane reads of one value at lanes L and L ^ 32
/// (a split-wave64 reduction tail) into one exchange (xcl 9e2c7a5
/// `fold_x64_readlane_tail`, GPL-2.0, copied with credit: both halves get
/// both lanes from the first exchange). The first read must not overwrite
/// the value the second reads. TB_WAVE64_FOLD=0 keeps two exchanges.
pub(super) fn fold_pairs(body: &str) -> String {
    if std::env::var("TB_WAVE64_FOLD").is_ok_and(|v| v.trim() == "0") {
        return body.to_string();
    }
    let lines: Vec<&str> = body.split_inclusive('\n').collect();
    let mut out = String::with_capacity(body.len());
    let mut i = 0;
    while i < lines.len() {
        let pair = (lane_read(lines[i].trim_end_matches('\n')), lines.get(i + 1).and_then(|l| lane_read(l.trim_end_matches('\n'))));
        if let (Some((ind, d1, v1, k1)), Some((_, d2, v2, k2))) = pair {
            if v1 == v2 && k1 < 64 && k2 == (k1 ^ 32) && d1 != v1 && !v1.contains(d1) {
                out.push_str(&format!("{ind}{{ uint2 tb_xp = tb_x_read2(asuint({v1}), {k1}u); {d1} = tb_xp.x; {d2} = tb_xp.y; }}\n"));
                i += 2;
                continue;
            }
        }
        out.push_str(lines[i]);
        i += 1;
    }
    out
}
