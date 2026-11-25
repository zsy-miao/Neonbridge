// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later
// Supersteps for a mesh group of VW_GROUP_THREADS threads holding several console waves of VW_WAVE consecutive threads;
// written into the superstep form of a mesh program after rdna_prelude.hlsli. Every wave operation is a step of the
// whole group. A thread runs to its next wave operation, s_barrier or end; at an operation it
// deposits its value and bit (vw_ss_put) and stops; the group meets at a barrier (vw_ss_step); each thread stopped at an
// operation then reads its console wave's result. Threads at an s_barrier wait until no thread is at an operation, so
// the barrier holds as on the console. Buffers alternate by step: a value buffer is written again two steps later, a
// mask buffer three steps later, with a barrier between.
static const uint PC_WAVE = 0xfffffffdu;   // a thread stopped at a wave operation
static const uint PC_IDLE = 0x7ffffffeu;   // the dispatch case of a stopped thread: it leaves with its pc unchanged
#ifndef VW_SS_VALUES
#define VW_SS_VALUES 1   // values a thread deposits in one step: 1, or 4 for up to four readfirstlanes of one mask
#endif
groupshared uint g_ss_x[2][VW_GROUP_THREADS * VW_SS_VALUES];
groupshared uint g_ss_m[3][(VW_GROUP_THREADS + 31u) / 32u];
groupshared uint g_ss_ops[3], g_ss_live[3], g_ss_alloc[3];
static uint g_ss_gi, g_ss_step, g_ss_read, g_ss_arg;
static bool g_ss_sent, g_ss_allocated;   // this thread sent GS_ALLOC_REQ since its last step; some thread of the group has
uint vw_ss_base() { return g_ss_gi & ~(VW_WAVE - 1u); }   // the group index of this console wave's lane 0
void vw_ss_clear(uint b) {
    [unroll] for (uint i = 0; i < (VW_GROUP_THREADS + 31u) / 32u; i++) g_ss_m[b][i] = 0;
    g_ss_ops[b] = 0;
    g_ss_live[b] = 0;
    g_ss_alloc[b] = 0;
}
void vw_ss_begin(uint gi) {
    g_ss_gi = gi;
    g_ss_step = 0;
    g_ss_sent = false;
    g_ss_allocated = false;
    if (gi == 0) { vw_ss_clear(0); vw_ss_clear(1); vw_ss_clear(2); }
    GroupMemoryBarrierWithGroupSync();
}
void vw_ss_put(uint x, bool bit, uint arg) {   // before stopping at a wave operation
    g_ss_x[g_ss_step & 1u][g_ss_gi * VW_SS_VALUES] = x;
    if (bit) InterlockedOr(g_ss_m[g_ss_step % 3u][g_ss_gi >> 5], 1u << (g_ss_gi & 31u));
    g_ss_arg = arg;
}
// One step for every thread: pc is PC_WAVE (deposited), PC_YIELD (at an s_barrier) or PC_END. Returns false once every
// thread has ended. A thread at a wave operation resumes; one at an s_barrier resumes when no thread is at an operation.
bool vw_ss_step(inout uint pc, uint pc_resume) {
    uint b = g_ss_step % 3u;
    if (g_ss_gi == 0) vw_ss_clear((b + 1u) % 3u);
    if (pc == PC_WAVE) InterlockedAdd(g_ss_ops[b], 1u);
    if (pc != PC_END) InterlockedAdd(g_ss_live[b], 1u);
    if (g_ss_sent) { g_ss_alloc[b] = 1u; g_ss_sent = false; }
    GroupMemoryBarrierWithGroupSync();
    uint ops = g_ss_ops[b], live = g_ss_live[b];
    g_ss_allocated = g_ss_allocated || g_ss_alloc[b] != 0u;
    g_ss_read = g_ss_step;
    g_ss_step++;
    if (pc == PC_WAVE || (pc == PC_YIELD && ops == 0u)) pc = pc_resume;
    return live != 0u;
}
// Whether this step released the threads at an s_barrier (every thread was at one or had ended).
bool vw_ss_released() { return g_ss_ops[g_ss_read % 3u] == 0u; }
// Whether a thread of the group sent GS_ALLOC_REQ before this step: read from the step's own buffer, so every thread
// answers the same (a flag a thread sets after the step could reach a slower thread still reading it).
bool vw_ss_allocated() { return g_ss_allocated; }
uint2 vw_ss_ballot() {
    uint w = (vw_ss_base() >> 5);
    uint b = g_ss_read % 3u;
    return VW_WAVE == 32 ? uint2(g_ss_m[b][w], 0u) : uint2(g_ss_m[b][w], g_ss_m[b][w + 1u]);
}
bool vw_ss_any() { uint2 m = vw_ss_ballot(); return (m.x | m.y) != 0; }
uint vw_ss_count() { uint2 m = vw_ss_ballot(); return countbits(m.x) + countbits(m.y); }
uint vw_ss_lane_value(uint lane) { return g_ss_x[g_ss_read & 1u][(vw_ss_base() + (lane & (VW_WAVE - 1u))) * VW_SS_VALUES]; }
uint vw_ss_read() { return vw_ss_lane_value(g_ss_arg); }
uint vw_ss_first() {   // readfirstlane: the first lane whose bit was set, lane 0 if none
    uint2 m = vw_ss_ballot();
    uint l = m.x != 0 ? firstbitlow(m.x) : m.y != 0 ? 32 + firstbitlow(m.y) : 0u;
    return vw_ss_lane_value(l);
}
#if VW_SS_VALUES == 4
void vw_ss_put_values(uint4 x, bool bit) {   // before stopping at up to VW_SS_VALUES readfirstlanes of one mask
    uint at = g_ss_gi * 4u;
    g_ss_x[g_ss_step & 1u][at] = x.x;
    g_ss_x[g_ss_step & 1u][at + 1u] = x.y;
    g_ss_x[g_ss_step & 1u][at + 2u] = x.z;
    g_ss_x[g_ss_step & 1u][at + 3u] = x.w;
    if (bit) InterlockedOr(g_ss_m[g_ss_step % 3u][g_ss_gi >> 5], 1u << (g_ss_gi & 31u));
    g_ss_arg = 0;
}
uint vw_ss_first_value(uint k) {   // value k of the first lane whose bit was set, lane 0 if none
    uint2 m = vw_ss_ballot();
    uint l = m.x != 0 ? firstbitlow(m.x) : m.y != 0 ? 32 + firstbitlow(m.y) : 0u;
    return g_ss_x[g_ss_read & 1u][(vw_ss_base() + l) * VW_SS_VALUES + k];
}
#endif