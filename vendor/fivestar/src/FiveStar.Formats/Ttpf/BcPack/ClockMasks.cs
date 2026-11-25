// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.CompilerServices;
using System.Runtime.Intrinsics.X86;

namespace FiveStar.Formats.Ttpf.BcPack;

/// <summary>
/// The feeder's decisions for one clock, as lane bit masks. A lane is granted a byte when it requests one, is not
/// blocked (it holds a symbol in a full slot) and fewer than min(8, bytes left) lanes before it were granted: the
/// first min(8, left) set bits of the candidates, one PDEP. A lane runs its phases unless it is blocked or was refused
/// a byte it needs.
/// </summary>
/// <param name="Grant">Lanes that receive the next stream bytes, in lane order.</param>
/// <param name="Push">Running lanes that hand their symbol over.</param>
/// <param name="Slow">Running lanes in IDLE or a header state: they take the shared control step.</param>
/// <param name="Commit">Running lanes in PROCESSING or DRAINING: the vector step writes their registers.</param>
internal readonly record struct ClockMasks(uint Grant, uint Push, uint Slow, uint Commit)
{
    /// <summary>The masks for lanes that <paramref name="need"/> a byte, given the held symbols and the slots.</summary>
    [HotPath]
    [MethodImpl(MethodImplOptions.AggressiveInlining)]
    public static ClockMasks Compute(uint need, uint symbolFull, uint slots, int available, uint vectorLanes, uint allLanes)
    {
        var blocked = symbolFull & slots;
        var grant = Bmi2.ParallelBitDeposit((1u << available) - 1, need & ~blocked);
        var go = allLanes & ~((need & ~grant) | blocked);

        return new ClockMasks(grant, go & symbolFull, go & ~vectorLanes, go & vectorLanes);
    }
}
