// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later
// Runtime support for translated RDNA programs.
// Bindless model: a descriptor's dword0 written by the D3D12.X shim is a PC descriptor-heap index (see GPU addresses).

#ifdef FIVESTAR_USER_CONSTANTS
// User SGPRs as 32 root constants (s0..s31: every user SGPR RDNA preloads); none past them, read as 0.
cbuffer UserData : register(b0) { uint4 g_user4[8]; };
uint user_sgpr(uint i) { return i < 32u ? g_user4[(i >> 2) & 7u][i & 3] : 0u; }
#else
cbuffer UserData : register(b0) { uint4 g_user4[32]; };       // initial user SGPRs (a root CBV of 128 dwords)
uint user_sgpr(uint i) { return g_user4[i >> 2][i & 3]; }
#endif
static const uint GPU_MEM_HEAP = 0;                            // raw window of the loader's 32-bit space, RW view at + 1
static const uint GPU_MAP_HEAP = 7;                            // address map of game memory (raw SRV)
static const uint VIEW_SLOTS = 1000000u;                       // the desktop view heap (DescriptorWindow.MaxViews)
static const uint SAMPLER_SLOTS = 2048u;                       // the desktop sampler heap (DescriptorWindow.MaxSamplers)
static const uint GPU_MAP_SPANS = 0x8000u;                     // 4 GiB spans of the 47-bit address space
static const uint STATIC_SAMPLER_BASE = 1024;                  // runtime-created samplers for literal S#s (StaticSampler)
static const uint PC_END = 0xffffffffu;
static const uint PC_YIELD = 0xfffffffeu;                      // mesh shaders: s_barrier yield point
static const uint CODE_TAG = 0xc0de0000u;                       // pointer hi-word for PC-relative (embedded) data

// A descriptor index a program read from memory names a slot inside its heap: past the end it names the null SRV or
// UAV the address map uses for unmapped pages, and a sampler past the end the last sampler. Without the clamp a stray
// index reads outside the heap (VIctory's port of the same game saw NVIDIA hang and AMD fault on it).
uint view_slot(uint i) { return i < VIEW_SLOTS ? i : GPU_MAP_HEAP + 1u; }
uint uav_slot(uint i) { return i < VIEW_SLOTS ? i : GPU_MAP_HEAP + 2u; }
// A texture's heap slot: a game view, or else (a zero or unwritten T#, a reserved buffer slot, past the heap) the null
// texture of its dimension at null_slot (NullTextureSlots), never a buffer read as a texture. The loader defines
// FIVESTAR_FIRST_GAME_VIEW (DescriptorWindow.ReservedViews, NullTextureSlots.End); without it every index below the heap's end is a view.
#ifndef FIVESTAR_FIRST_GAME_VIEW
#define FIVESTAR_FIRST_GAME_VIEW 0u
#endif
uint tex_slot(uint i, uint null_slot) { return (i >= FIVESTAR_FIRST_GAME_VIEW && i < VIEW_SLOTS) ? i : null_slot; }
uint sampler_slot(uint i) { return min(i, SAMPLER_SLOTS - 1u); }

uint MemLoad(uint ptr_lo, uint off) {
    ByteAddressBuffer m = ResourceDescriptorHeap[GPU_MEM_HEAP];
    return m.Load((ptr_lo + off) & ~3u);
}
void MemStore(uint ptr_lo, uint off, uint v) {
    RWByteAddressBuffer m = ResourceDescriptorHeap[GPU_MEM_HEAP + 1];   // writable view of the GPU-memory window
    m.Store((ptr_lo + off) & ~3u, v);
}

// ---- GPU addresses. A 64-bit pointer or V# base whose high dword is 0 is in the loader's 32-bit space: the window
// for a pointer, a heap index for a V# (the shim's descriptor records). Any other address is a console GPU address in
// game memory, found in the address map the shim keeps: level 1 holds per 4 GiB span (high dword) the byte offset of
// its page table; a page table holds per 64 KiB page the heap index of a raw SRV over the page (its UAV at + 1) and
// the page's byte offset in that view. Unmapped spans and pages name a null SRV/UAV pair. The view an access gets can
// differ between the lanes of a wave (lanes in different pages), so every heap access through one is
// NonUniformResourceIndex.
// With the loader's --evict-windows (FIVESTAR_WINDOW_FEEDBACK is the byte offset of its flags in the window), a page
// that waits for its section window's heap names the null views with 0x80000000 | window id in its offset field: the
// access reads zeros and sets the window's flag, so the loader opens that window (WindowFeedback).
uint2 gpu_page(uint lo, uint hi) {
    ByteAddressBuffer map = ResourceDescriptorHeap[GPU_MAP_HEAP];
    uint table = map.Load((hi & (GPU_MAP_SPANS - 1u)) * 4u);
    uint2 page = map.Load2(table + (lo >> 16) * 8u);
#ifdef FIVESTAR_WINDOW_FEEDBACK
    if (page.x == GPU_MAP_HEAP + 1u && (page.y & 0x80000000u) != 0u) MemStore(FIVESTAR_WINDOW_FEEDBACK, (page.y & 0xffffu) * 4u, 1u);
#endif
    return uint2(page.x, page.y + (lo & 0xffffu));
}
// 64-bit address arithmetic as the hardware's: a game-memory pointer plus an unsigned and a signed 32-bit addend
// carries into (or borrows from) the high dword; the window (high dword 0) stays in its 32-bit space.
uint2 ptr_add(uint lo, uint hi, uint u, int s) {
    uint a = lo + u;
    uint b = a + asuint(s);
    if (hi == 0u) return uint2(b, 0u);
    uint h = hi + (a < lo ? 1u : 0u);
    return uint2(b, h + (s < 0 ? 0xffffffffu : 0u) + (b < a ? 1u : 0u));
}
// ---- Store trace (loader --trace-pool-stores, which defines FIVESTAR_STORE_TRACE as the program's id): global stores
// and atomics into up to 8 address ranges, and buffer stores and atomics through up to 8 views, are appended to a ring
// at the end of the GPU-memory window. Header at TRACE_BASE: count, then 8 ranges {lo, hi, bytes} (bytes 0xffffffff:
// every address whose high dword is in [lo, hi]) from + 16, 8 view
// heap indices from + 112; entries of {program id, address lo or byte offset, address hi or heap index | kind << 31,
// value} from + 256. TRACE_BASE is DescriptorWindow.TraceBase.
#ifdef FIVESTAR_STORE_TRACE
static const uint TRACE_BASE = 0x04016100u;
static const uint TRACE_ENTRIES = 65536u;
void trace_entry(uint lo, uint hi, uint v) {
    RWByteAddressBuffer m = ResourceDescriptorHeap[GPU_MEM_HEAP + 1];
    uint i;
    m.InterlockedAdd(TRACE_BASE, 1u, i);
    m.Store4(TRACE_BASE + 256u + (i % TRACE_ENTRIES) * 16u, uint4(FIVESTAR_STORE_TRACE, lo, hi, v));
}
#endif
void TRACE_ADDR(uint lo, uint hi, uint v) {
#ifdef FIVESTAR_STORE_TRACE
    ByteAddressBuffer m = ResourceDescriptorHeap[GPU_MEM_HEAP];
    for (uint r = 0u; r < 8u; r++) {
        uint3 range = m.Load3(TRACE_BASE + 16u + r * 12u);
        bool span = range.z == 0xffffffffu && hi >= range.x && hi <= range.y;   // a range of high dwords
        if (span || (range.z != 0u && range.z != 0xffffffffu && hi == range.y && lo - range.x < range.z)) { trace_entry(lo, hi, v); return; }
    }
#endif
}
void TRACE_VSTORE(uint d0, uint d1, uint off, uint v) {   // a buffer store through a V# whose base is a game address
#ifdef FIVESTAR_STORE_TRACE
    uint hi = d1 & 0xffffu;
    if (hi == 0u) return;
    uint2 a = ptr_add(d0, hi, off, 0);
    TRACE_ADDR(a.x, a.y, v);
#endif
}void TRACE_SBUF(uint off, uint bytes, uint limit) {   // an s_buffer_load reaching past num_records (reads 0)
#ifdef FIVESTAR_STORE_TRACE
    if (off + bytes > limit && WaveIsFirstLane()) trace_entry(off, 0x20000000u | bytes, limit);
#endif
}void TRACE_GUARD(uint guard, uint cap) {   // structured loop number guard spent the invocation's loop cap
#ifdef FIVESTAR_STORE_TRACE
    trace_entry(0u, 0x40000000u | guard, cap);
#endif
}void TRACE_VIEW(uint heap_index, uint off, uint v) {
#ifdef FIVESTAR_STORE_TRACE
    ByteAddressBuffer m = ResourceDescriptorHeap[GPU_MEM_HEAP];
    for (uint r = 0u; r < 8u; r++) {
        if (m.Load(TRACE_BASE + 112u + r * 4u) == heap_index && heap_index != 0u) { trace_entry(off, heap_index | 0x80000000u, v); return; }
    }
#endif
}
// ---- Window guard (the loader's default; FIVESTAR_WINDOW_GUARD is the byte offset of its table in the window): a
// store or atomic through a 32-bit pointer (high dword 0) would land in the descriptor window, whose records and debug
// rings programs never write. It is dropped (an atomic goes to the sink after the table) and counted in slot
// FIVESTAR_PROGRAM_ID & 1023 of the table: {program id, drops}.
#ifdef FIVESTAR_WINDOW_GUARD
#ifndef FIVESTAR_PROGRAM_ID
#define FIVESTAR_PROGRAM_ID 0u
#endif
void window_guard_count() {
    RWByteAddressBuffer m = ResourceDescriptorHeap[GPU_MEM_HEAP + 1];
    uint slot = FIVESTAR_WINDOW_GUARD + (FIVESTAR_PROGRAM_ID & 1023u) * 8u;
    m.Store(slot, FIVESTAR_PROGRAM_ID);
    m.InterlockedAdd(slot + 4u, 1u);
}
#endif
// ---- Caps (with the window guard, FIVESTAR_CAP_TABLE is the byte offset of its table in the window): a thread that
// ran out of superstep blocks (kind 0), a group that ran out of rounds (kind 1, counted by thread 0) and a wave whose
// lanes spent the loop cap (kind 2, counted by its first active lane) are counted in slot FIVESTAR_PROGRAM_ID & 1023:
// {program id, threads past the block cap, groups past the round bound, waves past the loop cap}. A cap keeps the
// device alive; the loader names every program that reaches one, so the input or translation behind it gets fixed.
#ifdef FIVESTAR_CAP_TABLE
void ss_cap_hit(uint kind) {
    RWByteAddressBuffer m = ResourceDescriptorHeap[GPU_MEM_HEAP + 1];
    uint slot = FIVESTAR_CAP_TABLE + (FIVESTAR_PROGRAM_ID & 1023u) * 16u;
    m.Store(slot, FIVESTAR_PROGRAM_ID);
    m.InterlockedAdd(slot + 4u + kind * 4u, 1u);
}
#else
void ss_cap_hit(uint kind) {}
#endif
void loop_cap_hit(uint guard, uint cap) {   // structured loop number guard spent the invocation's loop cap
    TRACE_GUARD(guard, cap);
    if (WaveIsFirstLane()) ss_cap_hit(2u);
}
uint PtrLoad(uint lo, uint hi, uint off) {   // off is signed (a dword offset or an SMEM offset)
    uint2 a = ptr_add(lo, hi, 0u, asint(off));
    if (hi == 0u) return MemLoad(a.x, 0u);
    uint2 p = gpu_page(a.x, a.y);
    ByteAddressBuffer b = ResourceDescriptorHeap[NonUniformResourceIndex(p.x)];
    return b.Load(p.y & ~3u);
}
// An S#'s dword through a 32-bit pointer: relative to the sampler heap, whose records start at SAMPLER_WINDOW_BASE in
// the window (DescriptorWindow.SamplerBase); a view's 32-bit pointer is relative to the view heap, whose records start
// at 0. Scaleform's FTexTG programs read their S# at a root constant + index * 16 (SamplerLoads).
static const uint SAMPLER_WINDOW_BASE = 0x04008000u;
uint SamplerPtrLoad(uint lo, uint hi, uint off) {
    if (hi == 0u) return MemLoad(SAMPLER_WINDOW_BASE + lo, off);
    return PtrLoad(lo, hi, off);
}
void PtrStore(uint lo, uint hi, uint off, uint v) {
    uint2 a = ptr_add(lo, hi, 0u, asint(off));
#ifdef FIVESTAR_WINDOW_GUARD
    if (hi == 0u) { window_guard_count(); return; }
#else
    if (hi == 0u) { MemStore(a.x, 0u, v); return; }
#endif
    TRACE_ADDR(a.x, a.y, v);
    uint2 p = gpu_page(a.x, a.y);
    RWByteAddressBuffer b = ResourceDescriptorHeap[NonUniformResourceIndex(p.x + 1u)];
    b.Store(p.y & ~3u, v);
}
uint2 gpu_rw(uint lo, uint hi) {   // (RW heap index, byte address) of a pointer, for atomics
#ifdef FIVESTAR_WINDOW_GUARD
    if (hi == 0u) { window_guard_count(); return uint2(GPU_MEM_HEAP + 1u, FIVESTAR_WINDOW_GUARD + 1024u * 8u); }
#else
    if (hi == 0u) return uint2(GPU_MEM_HEAP + 1u, lo);
#endif
    uint2 p = gpu_page(lo, hi);
    return uint2(p.x + 1u, p.y);
}// An access's view: (heap index, byte offset of base + off in it); rw takes the UAV of a game-memory base. Dword1
// holds base_hi in bits 0-15 and the stride in bits 16-29. A game-memory access is looked up at its own address, as
// the hardware translates each page: the pages after the base's need not continue its view (another buffer, or
// frames the game remapped). The access's later dwords continue in the view of its first byte's page.
uint2 buf_view(uint d0, uint d1, bool rw, uint off) {
    uint hi = d1 & 0xffffu;
    if (hi == 0u) return uint2(d0, off);
    uint2 a = ptr_add(d0, hi, off, 0);
    uint2 p = gpu_page(a.x, a.y);
    return uint2(p.x + (rw ? 1u : 0u), p.y);
}
uint buf_stride(uint d1) { return (d1 >> 16) & 0x3fffu; }

// RDNA range checking: an out-of-range buffer load reads 0, and an out-of-range store or atomic is dropped. Programs
// rely on it (append and argument writers run past their allocation, loops read zeros past an end). By OOB_SELECT (V#
// dword3 bits 28-29), as measured on RDNA1 and RDNA2 hardware (Mesa's GFX11 column):
//   0 structured: index >= num_records, or offset + 4 > stride;  1 structured: index >= num_records;
//   2: num_records == 0;  3 raw: offset + soffset + 4 > num_records (index * stride moves the address, unchecked).
// The hardware adds index * stride to the 48-bit base without wrapping, so an index whose product reaches 4 GiB (an
// index of -1 from a counter at 0) lands far past any buffer; in the 32-bit offset of a view it would wrap to just
// below the base (20 bytes below for a 20-byte record), so such an access is out of range in every mode.
// num_records is dword2; the offset is the VGPR offset plus the instruction offset. Once per access buf_limit gives
// the end the dwords must fit under and buf_offset the checked offset; each dword then costs one compare (buf_fits).
uint buf_limit(uint d1, uint d2, uint d3, uint index) {
    uint sel = (d3 >> 28) & 3u;
    uint structured = select(index < d2, select(sel == 0u, buf_stride(d1), 0xffffffffu), 0u);
    uint unchecked = select(d2 == 0u, 0u, 0xffffffffu);
    uint limit = select(sel == 3u, d2, select(sel == 2u, unchecked, structured));
    uint stride = buf_stride(d1);
    return select(stride != 0u && index > 0xffffffffu / max(stride, 1u), 0u, limit);
}
uint buf_offset(uint d3, uint offset, uint soffset) {
    return select(((d3 >> 28) & 3u) == 3u, offset + soffset, offset);
}
bool buf_fits_bytes(uint start, uint bytes, uint limit) { return limit >= bytes && start <= limit - bytes; }   // a sub-dword access
bool buf_fits(uint start, uint limit) { return limit >= 4u && start <= limit - 4u; }
// s_buffer_load: the whole offset against num_records, counted in bytes (stride 0) or in strides.
uint sbuf_limit(uint d1, uint d2) {
    uint stride = max(buf_stride(d1), 1u);
    return select(d2 > 0xffffffffu / stride, 0xffffffffu, d2 * stride);
}

uint BufLoad(uint heap_index, uint off) {
    ByteAddressBuffer b = ResourceDescriptorHeap[NonUniformResourceIndex(view_slot(heap_index))];
    return b.Load(off & ~3u);
}
uint BufLoadRW(uint heap_index, uint off) {   // load through a UAV slot (buffer also written by this shader)
    RWByteAddressBuffer b = ResourceDescriptorHeap[NonUniformResourceIndex(uav_slot(heap_index))];
    return b.Load(off & ~3u);
}
void BufStore(uint heap_index, uint off, uint v) {
    TRACE_VIEW(heap_index, off, v);
    RWByteAddressBuffer b = ResourceDescriptorHeap[NonUniformResourceIndex(uav_slot(heap_index))];
    b.Store(off & ~3u, v);
}

// ---- program waves. Translated code numbers lanes as the console does: 0 .. VW_WAVE-1 of a VW_WAVE-lane wave. The vw_*
// operations take and return those numbers. With VW_EMULATE they stay correct on any hardware wave size:
//   hardware wave = program wave: the native operation;
//   wider: each aligned VW_WAVE-lane segment of the hardware wave is one program wave;
//   narrower: the hardware wave holds program lanes [origin, origin + WaveGetLaneCount()); the other lanes are missing
//   and read as 0 (on pixel and vertex waves that is a partly filled console wave).
// WaveGetLaneCount() is a constant once the driver compiles the program, so only one path remains.
// VW_GROUP_LANES (mesh): the program lane comes from SV_GroupIndex (g_vw_lane), set at entry.
// VW_GROUP_WAVE (compute, VW_GROUP_SIZE <= VW_WAVE threads): the thread group is the console wave. Every operation
// exchanges through groupshared memory with a barrier, so it is right on any hardware wave, but every thread of the
// group must reach every operation: the translated control flow is group-uniform because each branch condition is
// one of these operations, and the translator rejects programs that call one under a per-lane condition.
#ifndef VW_WAVE
#define VW_WAVE 64
#endif
#ifndef VW_EMULATE
#define VW_EMULATE 0
#endif
#ifndef VW_GROUP_LANES
#define VW_GROUP_LANES 0
#endif
#ifndef VW_GROUP_WAVE
#define VW_GROUP_WAVE 0
#endif
#if VW_GROUP_LANES || VW_GROUP_WAVE
static uint g_vw_lane;
#endif
#if VW_GROUP_WAVE
// A buffer is used again two operations of its kind later; the barrier of the operation between keeps it safe.
groupshared uint g_vw_x[2][VW_WAVE];   // values: one buffer per read, alternating
groupshared uint g_vw_m[3][2];         // masks: one in use, the next being cleared, the last still being read
static uint g_vw_reads, g_vw_ballots;  // operations so far (equal on every thread)
void vw_begin(uint gi) {
    g_vw_lane = gi;
    g_vw_reads = 0;
    g_vw_ballots = 0;
    if (gi == 0) { [unroll] for (uint i = 0; i < 3; i++) { g_vw_m[i][0] = 0; g_vw_m[i][1] = 0; } }
    GroupMemoryBarrierWithGroupSync();
}
uint vw_group_read(uint x, uint lane) {
    uint b = g_vw_reads & 1u;
    g_vw_reads++;
    g_vw_x[b][g_vw_lane] = x;
    GroupMemoryBarrierWithGroupSync();
    lane &= VW_WAVE - 1u;
    return lane < VW_GROUP_SIZE ? g_vw_x[b][lane] : 0u;
}
uint2 vw_group_ballot(bool c) {
    uint b = g_vw_ballots % 3u;
    g_vw_ballots++;
    if (g_vw_lane == 0) { g_vw_m[(b + 1u) % 3u][0] = 0; g_vw_m[(b + 1u) % 3u][1] = 0; }
    if (c) InterlockedOr(g_vw_m[b][g_vw_lane >> 5], 1u << (g_vw_lane & 31u));
    GroupMemoryBarrierWithGroupSync();
    return uint2(g_vw_m[b][0], g_vw_m[b][1]);
}
#endif

uint vw_lane() {
#if VW_GROUP_LANES || VW_GROUP_WAVE
    return g_vw_lane;
#elif VW_EMULATE
    return WaveGetLaneIndex() & (VW_WAVE - 1u);
#else
    return WaveGetLaneIndex();
#endif
}
// hardware lane of program lane 0 (wraps below 0 when the hardware wave is narrower and starts later in the program wave)
uint vw_origin() { return WaveGetLaneIndex() - vw_lane(); }
// lanes a program-lane loop visits
uint vw_lanes() {
#if VW_EMULATE || VW_GROUP_WAVE
    return VW_WAVE;
#else
    return WaveGetLaneCount();
#endif
}
uint vw_read(uint x, uint lane) {   // x of program lane `lane` (AMD uses the low log2(wave) bits)
#if VW_GROUP_WAVE
    return vw_group_read(x, lane);
#elif VW_EMULATE
    lane &= VW_WAVE - 1u;
    if (!VW_GROUP_LANES && WaveGetLaneCount() == VW_WAVE) return WaveReadLaneAt(x, lane);
    uint hw = vw_origin() + lane, count = WaveGetLaneCount();
    uint v = WaveReadLaneAt(x, min(hw, count - 1u));
    return hw < count ? v : 0u;
#else
    return WaveReadLaneAt(x, lane);
#endif
}
uint vw_read_fill(uint x, uint lane, uint fill) {   // vw_read where a missing lane holds fill (a lane without a pixel)
#if VW_EMULATE && !VW_GROUP_LANES
    lane &= VW_WAVE - 1u;
    uint hw = vw_origin() + lane, count = WaveGetLaneCount();
    uint v = WaveReadLaneAt(x, min(hw, count - 1u));
    return hw < count ? v : fill;
#else
    return vw_read(x, lane);
#endif
}
uint2 vw_ballot(bool c) {   // lane mask in program lanes (y = lanes 32..63)
#if VW_GROUP_WAVE
    return vw_group_ballot(c);
#endif
    uint4 b = WaveActiveBallot(c);
#if VW_EMULATE
    if (!VW_GROUP_LANES && WaveGetLaneCount() <= VW_WAVE) return b.xy;
    uint o = vw_origin();
    uint2 m;
    if (o < WaveGetLaneCount()) {   // wider: the program wave is the segment from hardware lane o
        uint w = o >> 5, s = o & 31u;
        uint3 t = uint3(b[w], b[min(w + 1u, 3u)], b[min(w + 2u, 3u)]);
        m = s == 0u ? t.xy : uint2((t.x >> s) | (t.y << (32u - s)), (t.y >> s) | (t.z << (32u - s)));
    } else {   // narrower: the hardware lanes (all in b.x) are program lanes s ..
        uint s = 0u - o;
        m = s >= 32u ? uint2(0u, b.x << (s - 32u)) : uint2(b.x << s, b.x >> (32u - s));
    }
    if (VW_WAVE < 32) return uint2(m.x & ((1u << (VW_WAVE & 31u)) - 1u), 0u);   // test-sized program waves
    return VW_WAVE == 32 ? uint2(m.x, 0u) : m;
#else
    return b.xy;
#endif
}
bool vw_any(bool c) {
#if VW_GROUP_WAVE
    uint2 g = vw_group_ballot(c); return (g.x | g.y) != 0;
#elif VW_EMULATE
    if (WaveGetLaneCount() > VW_WAVE) { uint2 m = vw_ballot(c); return (m.x | m.y) != 0; }
#endif
    return WaveActiveAnyTrue(c);
}
uint vw_count(bool c) {
#if VW_GROUP_WAVE
    uint2 g = vw_group_ballot(c); return countbits(g.x) + countbits(g.y);
#elif VW_EMULATE
    if (WaveGetLaneCount() > VW_WAVE) { uint2 m = vw_ballot(c); return countbits(m.x) + countbits(m.y); }
#endif
    return WaveActiveCountBits(c);
}

// ---- cross-lane
// When the hardware wave holds no more than the program wave, the first lane with EXEC set is the first hardware lane
// with `active`: it is read under `if (active)` and spread to the wave with a max (the other lanes give 0). Reading it
// with WaveReadLaneAt at the ballot's lowest bit crashed AMD's compiler in CreatePipelineState (RX 7800 XT, driver
// 32.0.32029.2010) on a pixel program that does it seven times in a loop, through dxc and the DXIL backend alike.
uint readfirstlane(uint x, bool active) {   // first lane with EXEC set (not just the first HLSL-active lane); lane 0 if none
    uint2 m = vw_ballot(active);
#if !VW_GROUP_WAVE && !VW_GROUP_LANES
    if (!VW_EMULATE || WaveGetLaneCount() <= VW_WAVE) {
        if ((m.x | m.y) == 0u) return vw_read(x, 0u);
        uint v = 0u;
        if (active) v = WaveReadLaneFirst(x);
        return WaveActiveMax(v);
    }
#endif
    uint l = m.x != 0 ? firstbitlow(m.x) : m.y != 0 ? 32 + firstbitlow(m.y) : 0u;
#if VW_EMULATE && !VW_GROUP_WAVE && !VW_GROUP_LANES
    // A wider hardware wave holds at most 4 program waves (128 lanes, or WARP's 4 for test waves): a max per program
    // wave, one component each.
    if ((m.x | m.y) == 0u) return vw_read(x, 0u);
    uint s = vw_origin() / VW_WAVE, v = vw_lane() == l ? x : 0u;
    uint4 r = WaveActiveMax(uint4(s == 0u ? v : 0u, s == 1u ? v : 0u, s == 2u ? v : 0u, s == 3u ? v : 0u));
    return s == 0u ? r.x : s == 1u ? r.y : s == 2u ? r.z : r.w;
#else
    return vw_read(x, l);
#endif
}
bool lane_bit(uint lo, uint hi) {   // this lane's bit of an integer lane mask (wave64 layout)
    uint l = vw_lane();
    return ((l < 32 ? (lo >> l) : (hi >> (l - 32))) & 1u) != 0;
}
uint quad_perm(uint x, uint p0, uint p1, uint p2, uint p3) {
    uint q0 = QuadReadLaneAt(x, 0), q1 = QuadReadLaneAt(x, 1), q2 = QuadReadLaneAt(x, 2), q3 = QuadReadLaneAt(x, 3);
    uint l = WaveGetLaneIndex() & 3;
    uint sel = l == 0 ? p0 : l == 1 ? p1 : l == 2 ? p2 : p3;
    return sel == 0 ? q0 : sel == 1 ? q1 : sel == 2 ? q2 : q3;
}

// ---- v_cmp_class_f32: bit0 sNaN, 1 qNaN, 2 -inf, 3 -normal, 4 -denorm, 5 -0, 6 +0, 7 +denorm, 8 +normal, 9 +inf
bool cmp_class(float x, uint m) {
    uint u = asuint(x); bool neg = (u >> 31) != 0; uint e = (u >> 23) & 0xff, f = u & 0x7fffff;
    uint bit;
    if (e == 0xff) bit = f != 0 ? ((f & 0x400000) ? 1 : 0) : (neg ? 2 : 9);
    else if (e == 0) bit = f == 0 ? (neg ? 5 : 6) : (neg ? 4 : 7);
    else bit = neg ? 3 : 8;
    return ((m >> bit) & 1) != 0;
}

// ---- formatted buffer loads. code = layout*8 + numeric type (GCN dfmt/nfmt numbering), from the gfx10 unified format
// in V# dword3 bits 12-18 (buffer_load_format, buf_code) or from the instruction (tbuffer_load_format).
// Branch-free (select only, no switch or loop): FmtLoad is inlined at every call, and a program can hold thousands,
// each of which would otherwise add dozens of blocks and an [unroll] loop that dxc unrolls against the whole function.
// Fields per layout, components x..w: bit offset | width << 8 (width 0 = no component). AMD format names list fields
// from the MSB: 2_10_10_10 = R10G10B10A2 (R in the low bits), 10_11_11 = R11G11B10.
static const uint4 FMT_FIELDS[16] = {
    uint4(0, 0, 0, 0),
    uint4(0x800, 0, 0, 0),                      // 1: 8
    uint4(0x1000, 0, 0, 0),                     // 2: 16
    uint4(0x800, 0x808, 0, 0),                  // 3: 8_8
    uint4(0x2000, 0, 0, 0),                     // 4: 32
    uint4(0x1000, 0x1010, 0, 0),                // 5: 16_16
    uint4(0xb00, 0xb0b, 0xa16, 0),              // 6: 10_11_11
    uint4(0xa00, 0xb0a, 0xb15, 0),              // 7: 11_11_10
    uint4(0x200, 0xa02, 0xa0c, 0xa16),          // 8: 10_10_10_2
    uint4(0xa00, 0xa0a, 0xa14, 0x21e),          // 9: 2_10_10_10
    uint4(0x800, 0x808, 0x810, 0x818),          // 10: 8_8_8_8
    uint4(0x2000, 0x2020, 0, 0),                // 11: 32_32
    uint4(0x1000, 0x1010, 0x1020, 0x1030),      // 12: 16_16_16_16
    uint4(0x2000, 0x2020, 0x2040, 0),           // 13: 32_32_32
    uint4(0x2000, 0x2020, 0x2040, 0x2060),      // 14: 32_32_32_32
    uint4(0, 0, 0, 0),
};
uint fmt_field(uint4 w, uint field) {
    uint off = field & 0xffu, bits = field >> 8, d = off >> 5;
    uint word = select(d == 0u, w.x, select(d == 1u, w.y, select(d == 2u, w.z, w.w)));
    return (word >> (off & 31u)) & select(bits >= 32u, 0xffffffffu, (1u << (bits & 31u)) - 1u);
}
uint fmt_convert(uint v, uint bits, uint num) {
    uint mx = select(bits >= 32u, 0xffffffffu, (1u << (bits & 31u)) - 1u), sh = (32u - bits) & 31u;
    int sv = select(bits >= 32u, asint(v), asint(v << sh) >> sh);
    float un = (float)v / (float)mx;                                         // UNORM
    float sn = max((float)sv / (float)(mx >> 1), -1.0);                      // SNORM
    // FLOAT: 11- and 10-bit fields are unsigned halves with 6 and 5 mantissa bits
    float fp = select(bits == 32u, asfloat(v), f16tof32(v << select(bits == 11u, 4u, select(bits == 10u, 5u, 0u))));
    float f = select(num == 0u, un, select(num == 1u, sn, select(num == 2u, (float)v,   // USCALED
              select(num == 3u, (float)sv, select(num == 7u, fp, 0.0)))));                 // SSCALED
    return select(num == 4u, v, select(num == 5u, asuint(sv), asuint(f)));                 // UINT, SINT
}
uint fmt_component(uint4 w, uint field, uint num, uint missing) {
    return select((field >> 8) != 0u, fmt_convert(fmt_field(w, field), field >> 8, num), missing);
}
// DST_SEL_X..W (V# dword3 bits 0-11, d3): 0 and 1 the constants, 4-7 the loaded x..w (BGRA views read z, y, x, w);
// all four 0 keeps the loaded order. Each result component converts only the field its selector names, so a load
// whose program reads x converts one field rather than four (the unread components then fold away): converting every
// field and choosing after made the 3,281 loads of a 122k-dword compute program 41% of its DXIL instructions.
uint fmt_selected(uint4 w, uint4 fields, uint num, uint one, uint s, uint k) {
    uint sel = select(s == 0u, 4u + k, (s >> (3u * k)) & 7u), c = sel & 3u;
    uint field = select(c == 0u, fields.x, select(c == 1u, fields.y, select(c == 2u, fields.z, fields.w)));
    return select(sel >= 4u, fmt_component(w, field, num, select(c == 3u, one, 0u)), select(sel == 1u, one, 0u));
}
uint4 FmtLoad(uint heap_index, uint addr, uint code, uint d3) {
    ByteAddressBuffer b = ResourceDescriptorHeap[NonUniformResourceIndex(view_slot(heap_index))];
    uint base = addr & ~3u, sh = (addr & 3u) * 8u;
    uint4 w = b.Load4(base); uint w4 = b.Load(base + 16);
    uint4 shifted = (w >> sh) | (uint4(w.yzw, w4) << ((32u - sh) & 31u));
    w = select(sh != 0u, shifted, w);
    uint layout = code >> 3, num = code & 7u;
    uint4 fields = select(layout < 16u, FMT_FIELDS[layout & 15u], uint4(0, 0, 0, 0));
    uint one = select(num - 4u <= 1u, 1u, asuint(1.0)), s = d3 & 0xfffu;
    return uint4(fmt_selected(w, fields, num, one, s, 0u), fmt_selected(w, fields, num, one, s, 1u),
                 fmt_selected(w, fields, num, one, s, 2u), fmt_selected(w, fields, num, one, s, 3u));
}
// gfx10 unified buffer formats (RDNA 2 ISA, buffer data formats: 1 8_UNORM .. 77 32_32_32_32_FLOAT) as codes; 0 invalid.
static const uint FMT_UNIFIED[80] = {
    0, 8, 9, 10, 11, 12, 13, 16, 17, 18, 19, 20, 21, 23, 24, 25, 26, 27, 28, 29, 36, 37, 39, 40, 41, 42, 43, 44, 45, 47,
    48, 49, 50, 51, 52, 53, 55, 56, 57, 58, 59, 60, 61, 63, 64, 65, 66, 67, 68, 69, 72, 73, 74, 75, 76, 77, 80, 81, 82, 83,
    84, 85, 92, 93, 95, 96, 97, 98, 99, 100, 101, 103, 108, 109, 111, 116, 117, 119, 0, 0,
};
uint buf_code(uint d3) { uint f = (d3 >> 12) & 0x7fu; return select(f < 80u, FMT_UNIFIED[min(f, 79u)], 0u); }
// A format access checks its element (1 to 16 bytes, FMT_FIELDS' layouts) against the range, not a dword: an R16
// structured buffer of stride 2 is in range, where a dword check against its stride failed every access (the mesh
// probe of MS_VS_Transform_PositionOnly: 2,876 of 2,895 groups read 0 at that load). As a limit for buf_fits.
static const uint FMT_BYTES[16] = { 4, 1, 2, 2, 4, 4, 4, 4, 4, 4, 4, 8, 8, 12, 16, 4 };
uint fmt_limit(uint limit, uint code) {
    uint n = FMT_BYTES[(code >> 3) & 15u];
    return select(limit < n, 0u, select(limit > 0xfffffff0u, limit, limit - n + 4u));
}
// The d16 forms (buffer_load_format_d16_*, tbuffer_store_format_d16_*, ...) carry a component as a 16-bit half: an
// integer format's low 16 bits, any other format's value as f16; fmt_unhalf widens a stored half back (SINT sign-extended).
uint fmt_half(uint v, uint code) { return select((code & 7u) - 4u <= 1u, v & 0xffffu, f32tof16(asfloat(v))); }
uint fmt_unhalf(uint h, uint code) {
    uint num = code & 7u;
    return select(num == 4u, h, select(num == 5u, (h ^ 0x8000u) - 0x8000u, asuint(f16tof32(h))));
}

// ---- cube maps (AMD conventions). face: 0 +X, 1 -X, 2 +Y, 3 -Y, 4 +Z, 5 -Z
float cubeid(float x, float y, float z) {
    if (abs(z) >= abs(x) && abs(z) >= abs(y)) return z < 0 ? 5.0 : 4.0;
    if (abs(y) >= abs(x)) return y < 0 ? 3.0 : 2.0;
    return x < 0 ? 1.0 : 0.0;
}
float cubesc(float x, float y, float z) {
    if (abs(z) >= abs(x) && abs(z) >= abs(y)) return z < 0 ? -x : x;
    if (abs(y) >= abs(x)) return x;
    return x < 0 ? z : -z;
}
float cubetc(float x, float y, float z) {
    if (abs(z) >= abs(x) && abs(z) >= abs(y)) return -y;
    if (abs(y) >= abs(x)) return y < 0 ? -z : z;
    return -y;
}
float cubema(float x, float y, float z) {
    if (abs(z) >= abs(x) && abs(z) >= abs(y)) return 2.0 * z;
    if (abs(y) >= abs(x)) return 2.0 * y;
    return 2.0 * x;
}
// inverse of the hardware cube addressing: s,t in [1,2] (sc/|ma| + 1.5), face id -> direction
float3 cube_dir(float s, float t, float face) {
    float u = (s - 1.5) * 2.0, v = (t - 1.5) * 2.0;
    uint f = (uint)face % 6;
    switch (f) {
        case 0: return float3(1, -v, -u);
        case 1: return float3(-1, -v, u);
        case 2: return float3(u, 1, v);
        case 3: return float3(u, -1, -v);
        case 4: return float3(u, -v, 1);
        default: return float3(-u, -v, -1);
    }
}

uint enc_num(float f, uint bits, uint num) {
    uint mx = bits >= 32 ? 0xffffffffu : ((1u << bits) - 1);
    switch (num) {
        case 0: return (uint)round(saturate(f) * (float)mx);
        case 1: return asuint((int)round(clamp(f, -1.0, 1.0) * (float)(mx >> 1))) & mx;
        case 2: return (uint)f & mx;
        case 3: return asuint((int)f) & mx;
        case 7: return bits == 32 ? asuint(f) : bits == 16 ? f32tof16(f) : ((uint)f & mx);
    }
    return 0;
}
void FmtStore(uint heap_index, uint addr, uint code, uint4 v, uint n) {
    RWByteAddressBuffer b = ResourceDescriptorHeap[NonUniformResourceIndex(uav_slot(heap_index))];
    uint layout = code >> 3, num = code & 7;
    bool ints = num == 4 || num == 5;
    uint4 c = v;
    uint a = addr & ~3u;
    switch (layout) {
        case 4:  b.Store(a, ints ? c.x : enc_num(asfloat(v.x), 32, num)); break;
        case 5:  b.Store(a, (ints ? (c.x & 0xffff) | (c.y << 16) : enc_num(asfloat(v.x), 16, num) | (enc_num(asfloat(v.y), 16, num) << 16))); break;
        case 10: b.Store(a, ints ? ((c.x & 0xff) | ((c.y & 0xff) << 8) | ((c.z & 0xff) << 16) | (c.w << 24))
                                 : (enc_num(asfloat(v.x), 8, num) | (enc_num(asfloat(v.y), 8, num) << 8) | (enc_num(asfloat(v.z), 8, num) << 16) | (enc_num(asfloat(v.w), 8, num) << 24))); break;
        case 9:  b.Store(a, enc_num(asfloat(v.x), 10, num) | (enc_num(asfloat(v.y), 10, num) << 10) | (enc_num(asfloat(v.z), 10, num) << 20) | (enc_num(asfloat(v.w), 2, num) << 30)); break;
        case 11: b.Store2(a, ints ? c.xy : uint2(enc_num(asfloat(v.x), 32, num), enc_num(asfloat(v.y), 32, num))); break;
        case 12: b.Store2(a, ints ? uint2((c.x & 0xffff) | (c.y << 16), (c.z & 0xffff) | (c.w << 16))
                                  : uint2(enc_num(asfloat(v.x), 16, num) | (enc_num(asfloat(v.y), 16, num) << 16), enc_num(asfloat(v.z), 16, num) | (enc_num(asfloat(v.w), 16, num) << 16))); break;
        case 13: b.Store3(a, ints ? c.xyz : uint3(enc_num(asfloat(v.x), 32, num), enc_num(asfloat(v.y), 32, num), enc_num(asfloat(v.z), 32, num))); break;
        case 14: b.Store4(a, ints ? c : uint4(enc_num(asfloat(v.x), 32, num), enc_num(asfloat(v.y), 32, num), enc_num(asfloat(v.z), 32, num), enc_num(asfloat(v.w), 32, num))); break;
        default: break;   // 8-bit / 16-bit single-channel and packed float formats: TODO (need byte-granular RMW)
    }
}

float mul_legacy(float a, float b) { return (a == 0.0 || b == 0.0) ? 0.0 : a * b; }
float mad_legacy(float a, float b, float c) { return mul_legacy(a, b) + c; }
// V_MAD_F32 / V_MAC_F32 round the product before the add. precise keeps dxc from marking the two operations fast, which
// lets a driver contract them into one fused operation (NVIDIA does).
float mad_unfused(float a, float b, float c) { precise float p = a * b; precise float r = p + c; return r; }
// V_FRACT_F32: x - floor(x), clamped to the largest float below 1.0; x - floor(x) of a small negative x rounds to 1.0.
// A NaN stays NaN.
float fract_f32(float x) { float f = frac(x); return f >= 1.0 ? asfloat(0x3f7fffffu) : f; }
float med3(float a, float b, float c) { return max(min(a, b), min(max(a, b), c)); }
uint med3u(uint a, uint b, uint c) { return max(min(a, b), min(max(a, b), c)); }
int med3i(int a, int b, int c) { return max(min(a, b), min(max(a, b), c)); }
float round_ne(float x) { return round(x); }
uint bfe_u(uint v, uint off, uint w) { off &= 31; w &= 31; return w == 0 ? 0 : (v >> off) & ((1u << w) - 1); }
uint bfe_i(uint v, uint off, uint w) { off &= 31; w &= 31; if (w == 0) return 0; if (off + w >= 32) return asuint(asint(v) >> off); int s = asint(v << (32 - off - w)); return asuint(s >> (32 - w)); }

// exact f32 -> f16 conversions (f32tof16 rounding is implementation-defined; AMD default = round to nearest even)
uint f32tof16_rne(float f) {
    uint u = asuint(f); uint s = (u >> 16) & 0x8000u; uint a = u & 0x7fffffffu;
    if (a > 0x7f800000u) return s | 0x7e00u;
    if (a >= 0x477ff000u) return s | 0x7c00u;
    if (a < 0x38800000u) {
        if (a < 0x33000000u) return s;
        uint e = a >> 23; uint m = (a & 0x7fffffu) | 0x800000u; uint sh = 126u - e;
        uint q = m >> sh; uint rem = m & ((1u << sh) - 1u); uint hf = 1u << (sh - 1u);
        return s | (q + ((rem > hf || (rem == hf && (q & 1u))) ? 1u : 0u));
    }
    uint r = a - 0x38000000u; uint q = r >> 13; uint rem = r & 0x1fffu;
    return s | (q + ((rem > 0x1000u || (rem == 0x1000u && (q & 1u))) ? 1u : 0u));
}
uint f32tof16_rtz(float f) {
    uint u = asuint(f); uint s = (u >> 16) & 0x8000u; uint a = u & 0x7fffffffu;
    if (a > 0x7f800000u) return s | 0x7e00u;
    if (a == 0x7f800000u) return s | 0x7c00u;
    if (a >= 0x47800000u) return s | 0x7bffu;
    if (a < 0x38800000u) { if (a < 0x33800000u) return s; uint e = a >> 23; uint m = (a & 0x7fffffu) | 0x800000u; return s | (m >> (126u - e)); }
    return s | ((a - 0x38000000u) >> 13);
}
float ldexp_exact(float x, int e) { e = clamp(e, -378, 381); int a = e / 3; int b = (e - a) / 2; int c = e - a - b;
    precise float r = x * asfloat((uint)(a + 127) << 23); r = r * asfloat((uint)(b + 127) << 23); r = r * asfloat((uint)(c + 127) << 23); return r; }
// fp32 fused multiply-add via fp64 (HLSL has no fp32 fma; exact for fp32 inputs)
float fma32(float a, float b, float c) { return (float)((double)a * (double)b + (double)c); }
int sext24(uint v) { return asint(v << 8) >> 8; }
uint mul_hi_u(uint a, uint b) { uint64_t p = (uint64_t)a * (uint64_t)b; return (uint)(p >> 32); }
uint firstbithigh_amd(uint v) { return v == 0 ? 0xffffffffu : 31 - firstbithigh(v); }
// s_flbit_i32 / v_ffbh_i32: count of leading bits equal to the sign bit (from MSB); -1 if all bits are equal
uint flbit_signed(uint v) { uint x = (v >> 31) != 0 ? ~v : v; return x == 0 ? 0xffffffffu : 31 - firstbithigh(x); }
uint alignbit(uint hi, uint lo, uint s) { uint64_t x = ((uint64_t)hi << 32) | lo; return (uint)(x >> (s & 31)); }
uint perm_b32(uint a, uint b, uint sel) {
    uint64_t x = ((uint64_t)a << 32) | b; uint r = 0;
    for (uint i = 0; i < 4; i++) {
        uint s = (sel >> (8 * i)) & 0xff; uint byte;
        if (s < 8) byte = (uint)(x >> (8 * s)) & 0xff;
        else if (s == 12) byte = 0; else if (s >= 13) byte = 0xff;
        else byte = ((uint)(x >> (8 * ((s - 8) * 2 + 1))) & 0x80) ? 0xff : 0;
        r |= byte << (8 * i);
    }
    return r;
}

// ---- Mesh probe (loader --probe-pipeline; MeshProbe): FIVESTAR_MESH_PROBE is the program's id,
// FIVESTAR_MESH_PROBE_BASE the byte offset of its ring in the GPU-memory window (a count, then 384-byte records from
// + 16), FIVESTAR_MESH_PROBE_SITES its number of load sites. Each load site calls a probe_* form with its number, which
// marks the site (bit site & 31) in a per-thread mask when its range check fails (probe_buf_fits, probe_sbuf_fits),
// when it reads an unmapped page of the address map (the null pair GPU_MAP_HEAP + 1, + 2), or a record naming no
// descriptor (PROBE_NULL_RECORD, DescriptorWindow.NullViewSlot); the register a site's line loads is noted by thread 0
// the first time (probe_value), and m0 at GS_ALLOC_REQ (probe_alloc). At the end of the group thread 0 appends: {id,
// group x y z; vertices, primitives as SetMeshOutputCounts got them, culled primitives, cull mask of primitives 0-31;
// vertex 0's position; flags (bit 0: position written, bit 1: m0 noted), range, unmapped and null-record masks;
// the values of sites 0-31; the mask of values noted; m0; the number of sites; id ^ PROBE_MARK; the mask of sites
// that loaded through a 32-bit pointer (the window), the first such address thread 0 read (flags bit 2), the mask of
// sites with a window address noted; from + 256 the first window address thread 0 read at each of sites 0-31}.
#ifdef FIVESTAR_MESH_PROBE
static const uint PROBE_RECORDS = 4096u;
static const uint PROBE_RECORD_BYTES = 384u;
static const uint PROBE_NULL_RECORD = 5u;
static const uint PROBE_MARK = 0x50524f42u;   // a whole record's last dword: its id ^ this
static uint g_probe_range, g_probe_unmapped, g_probe_record, g_probe_window, g_probe_gi;
// A pixel or compute program (FIVESTAR_STAGE_PROBE) keeps its notes per invocation: each lane is its own thread 0.
#ifdef FIVESTAR_STAGE_PROBE
static uint g_probe_g[15];
static uint g_probe_v[32];
static uint g_probe_w[32];
#else
groupshared uint g_probe_g[15];   // range, unmapped, null record, culled, cull mask, flags, position x y z w, values mask, m0, window mask, window address, window sites noted
groupshared uint g_probe_v[32];
groupshared uint g_probe_w[32];   // per site: the first window address thread 0 read
#endif
uint probe_bit(uint site) { return 1u << min(site, 31u); }
bool probe_sbuf_fits(uint site, bool fits) {
    if (!fits) g_probe_range |= probe_bit(site);
    return fits;
}
bool probe_buf_fits(uint site, uint start, uint limit) { return probe_sbuf_fits(site, buf_fits(start, limit)); }
void probe_view(uint site, uint heap_index) {
    if (heap_index == GPU_MAP_HEAP + 1u || heap_index == GPU_MAP_HEAP + 2u) g_probe_unmapped |= probe_bit(site);
    if (heap_index == PROBE_NULL_RECORD) g_probe_record |= probe_bit(site);
}
uint probe_BufLoad(uint site, uint heap_index, uint off) { probe_view(site, heap_index); return BufLoad(heap_index, off); }
uint probe_BufLoadRW(uint site, uint heap_index, uint off) { probe_view(site, heap_index); return BufLoadRW(heap_index, off); }
uint4 probe_FmtLoad(uint site, uint heap_index, uint addr, uint code, uint d3) { probe_view(site, heap_index); return FmtLoad(heap_index, addr, code, d3); }
uint probe_PtrLoad(uint site, uint lo, uint hi, uint off) {
    if (hi != 0u) { uint2 a = ptr_add(lo, hi, 0u, asint(off)); probe_view(site, gpu_page(a.x, a.y).x); }
    else {   // a 32-bit pointer: the window (descriptor records); the first such address thread 0 reads is kept
        g_probe_window |= probe_bit(site);
        if (g_probe_gi == 0u && (g_probe_g[5] & 4u) == 0u) { g_probe_g[13] = lo + off; g_probe_g[5] |= 4u; }
        if (g_probe_gi == 0u && site < 32u && (g_probe_g[14] & (1u << site)) == 0u) { g_probe_w[site] = lo + off; g_probe_g[14] |= 1u << site; }
    }
    return PtrLoad(lo, hi, off);
}
void probe_value(uint site, uint v) {   // the register a site's line loaded, the first time on thread 0
    if (g_probe_gi != 0u || site >= 32u || (g_probe_g[10] & (1u << site)) != 0u) return;
    g_probe_v[site] = v;
    g_probe_g[10] |= 1u << site;
}
#ifdef FIVESTAR_STAGE_PROBE
// The end of a pixel or compute invocation: the first lane of each wave appends a record of its own notes, with the
// site masks of every lane of the wave, its group (compute) or pixel (x, y), and the colour it returns (pixel, flags
// bit 0) in the position fields; flags bit 3 marks a stage record.
void probe_end_stage(uint3 at_id, float4 colour, bool coloured) {
    uint range = WaveActiveBitOr(g_probe_range), unmapped = WaveActiveBitOr(g_probe_unmapped);
    uint record = WaveActiveBitOr(g_probe_record), window = WaveActiveBitOr(g_probe_window);
    if (!WaveIsFirstLane()) return;
    RWByteAddressBuffer m = ResourceDescriptorHeap[GPU_MEM_HEAP + 1];
    uint i;
    m.InterlockedAdd(FIVESTAR_MESH_PROBE_BASE, 1u, i);
    uint at = FIVESTAR_MESH_PROBE_BASE + 16u + (i % PROBE_RECORDS) * PROBE_RECORD_BYTES;
    uint flags = (g_probe_g[5] & 4u) | 8u | (coloured ? 1u : 0u);
    m.Store4(at, uint4(FIVESTAR_MESH_PROBE, at_id.x, at_id.y, at_id.z));
    m.Store4(at + 16u, uint4(0u, 0u, 0u, 0u));
    m.Store4(at + 32u, asuint(colour));
    m.Store4(at + 48u, uint4(flags, range, unmapped, record));
    [unroll] for (uint k = 0u; k < 32u; k += 4u) m.Store4(at + 64u + k * 4u, uint4(g_probe_v[k], g_probe_v[k + 1u], g_probe_v[k + 2u], g_probe_v[k + 3u]));
    m.Store4(at + 192u, uint4(g_probe_g[10], 0u, FIVESTAR_MESH_PROBE_SITES, FIVESTAR_MESH_PROBE ^ PROBE_MARK));
    m.Store3(at + 208u, uint3(window, g_probe_g[13], g_probe_g[14]));
    [unroll] for (uint w = 0u; w < 32u; w += 4u) m.Store4(at + 256u + w * 4u, uint4(g_probe_w[w], g_probe_w[w + 1u], g_probe_w[w + 2u], g_probe_w[w + 3u]));
}
#else
void probe_alloc(uint m0) {   // GS_ALLOC_REQ's m0, the first time
    if ((g_probe_g[5] & 2u) != 0u) return;
    g_probe_g[11] = m0;
    g_probe_g[5] |= 2u;
}
void probe_begin(uint gi) {   // before the group's first barrier
    g_probe_gi = gi;
    if (gi == 0u) { [unroll] for (uint i = 0u; i < 15u; i++) g_probe_g[i] = 0u; }
}
void probe_position(float4 p) {   // thread 0, writing vertex 0
    g_probe_g[5] |= 1u;
    g_probe_g[6] = asuint(p.x); g_probe_g[7] = asuint(p.y); g_probe_g[8] = asuint(p.z); g_probe_g[9] = asuint(p.w);
}
void probe_cull(uint prim, bool culled, uint np) {   // a primitive of the group's count (np) exported culled, once
    if (!culled || prim >= np) return;
    uint before = 0u;
    if (prim < 32u) InterlockedOr(g_probe_g[4], 1u << prim, before);
    if ((before & (1u << min(prim, 31u))) == 0u || prim >= 32u) InterlockedAdd(g_probe_g[3], 1u);
}
void probe_end(uint3 gid, uint gi, uint nv, uint np) {   // every thread of the group, at its end
    InterlockedOr(g_probe_g[0], g_probe_range);
    InterlockedOr(g_probe_g[1], g_probe_unmapped);
    InterlockedOr(g_probe_g[2], g_probe_record);
    InterlockedOr(g_probe_g[12], g_probe_window);
    GroupMemoryBarrierWithGroupSync();
    if (gi != 0u) return;
    RWByteAddressBuffer m = ResourceDescriptorHeap[GPU_MEM_HEAP + 1];
    uint i;
    m.InterlockedAdd(FIVESTAR_MESH_PROBE_BASE, 1u, i);
    uint at = FIVESTAR_MESH_PROBE_BASE + 16u + (i % PROBE_RECORDS) * PROBE_RECORD_BYTES;
    m.Store4(at, uint4(FIVESTAR_MESH_PROBE, gid.x, gid.y, gid.z));
    m.Store4(at + 16u, uint4(nv, np, g_probe_g[3], g_probe_g[4]));
    m.Store4(at + 32u, uint4(g_probe_g[6], g_probe_g[7], g_probe_g[8], g_probe_g[9]));
    m.Store4(at + 48u, uint4(g_probe_g[5], g_probe_g[0], g_probe_g[1], g_probe_g[2]));
    [unroll] for (uint k = 0u; k < 32u; k += 4u) m.Store4(at + 64u + k * 4u, uint4(g_probe_v[k], g_probe_v[k + 1u], g_probe_v[k + 2u], g_probe_v[k + 3u]));
    m.Store4(at + 192u, uint4(g_probe_g[10], g_probe_g[11], FIVESTAR_MESH_PROBE_SITES, FIVESTAR_MESH_PROBE ^ PROBE_MARK));
    m.Store3(at + 208u, uint3(g_probe_g[12], g_probe_g[13], g_probe_g[14]));
    [unroll] for (uint w = 0u; w < 32u; w += 4u) m.Store4(at + 256u + w * 4u, uint4(g_probe_w[w], g_probe_w[w + 1u], g_probe_w[w + 2u], g_probe_w[w + 3u]));
}
#endif
#endif