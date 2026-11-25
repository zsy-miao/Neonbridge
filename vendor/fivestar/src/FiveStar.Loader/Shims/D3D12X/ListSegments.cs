// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Shims.D3D12X;

/// <summary>
/// The desktop lists a command list was split into at its memory waits (<see cref="ListSplits"/>), kept in its wrapper:
/// the closed segments in order (the wrapper's inner list records the last one), the desktop lists earlier recordings
/// used as segments, kept to record again, and the desktop allocator the recording uses.
/// </summary>
[StructLayout(LayoutKind.Sequential)]
internal unsafe struct ListSegments
{
    /// <summary>Offset in the command-list wrapper, past the game's fields and the shim's other state (0x1000 on).</summary>
    public const int Offset = 0x1000;

    /// <summary>Segments a recording can close; a wait past them is not kept.</summary>
    public const int Capacity = 4;

    /// <summary>
    /// Offset of <see cref="Commands"/> in the wrapper: the forwarding thunks of command lists count every call there
    /// (<see cref="ForwardingThunk"/>), handlers that record work count themselves.
    /// </summary>
    public const int CommandsOffset = Offset;

    /// <summary>Commands recorded since the last Reset (forwarded calls and work-recording handlers).</summary>
    public int Commands;
    public int Count;
    public int Spares;
    public nint Allocator;
    public fixed long Closed[Capacity];
    public fixed long Spare[Capacity];

    public static ListSegments* Of(XboxObject* list)
    {
        return (ListSegments*)((byte*)list + Offset);
    }

    /// <summary>The desktop list of segment <paramref name="index"/>: a closed one, or <paramref name="inner"/> for the last.</summary>
    public readonly nint Segment(int index, nint inner)
    {
        if (index < Count)
        {
            return (nint)Closed[index];
        }

        return inner;
    }

    /// <summary>A new recording on <paramref name="allocator"/>: the closed segments become spares.</summary>
    [HotPath]
    public void Reset(nint allocator)
    {
        Allocator = allocator;
        Commands = 0;

        for (int i = 0; i < Count && Spares < Capacity; i++)
        {
            Spare[Spares++] = Closed[i];
        }

        Count = 0;
    }
}
