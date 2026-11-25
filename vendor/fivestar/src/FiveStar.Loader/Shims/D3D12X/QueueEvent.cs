// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X;

/// <summary>
/// One queue call: an execution (the shim fence value it ends at, its list count, the last pipeline of its last list
/// and the draws, dispatches and indirect executions of all its lists) or a Signal or Wait on a game fence.
/// </summary>
internal struct QueueEvent
{
    public long At;
    public QueueEventKind Kind;
    public uint Lists;
    public nint Fence;
    public ulong Value;
    public nint Pipeline;
    public int Draws;
    public int Dispatches;
    public int Indirect;
}
