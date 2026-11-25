// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Shims.D3D12X;

/// <summary>
/// The memory waits a command list recorded since its last Reset (slots 73 and 74, <see cref="MemoryWaits"/>), kept in
/// its wrapper with the segment each one precedes (<see cref="ListSegments"/>), and the last addresses its bottom-of-pipe
/// writes went to: a wait on an address the same list wrote before it is met by the list's own order and is not kept.
/// </summary>
[StructLayout(LayoutKind.Sequential)]
internal unsafe struct ListMemoryWaits
{
    /// <summary>Offset in the command-list wrapper, past <see cref="ListReplayState"/>.</summary>
    public const int Offset = 0x1600;

    /// <summary>Waits a recording keeps; later ones are counted in <see cref="Dropped"/>.</summary>
    public const int Capacity = 16;

    /// <summary>Write addresses remembered, the newest ones.</summary>
    public const int WriteCapacity = 32;

    public int Count;
    public int Dropped;
    public int Writes;

    /// <summary>Work (draws, dispatches, indirect executions and writes) recorded when the list was last split.</summary>
    public int WorkAtSplit;

    /// <summary>When the list was last submitted (Environment.TickCount64) and to which queue (<see cref="MemoryWaitWriters"/>).</summary>
    public long SubmittedAt;
    public int Queue;
    public fixed ulong Address[Capacity];
    public fixed ulong Value[Capacity];
    public fixed ulong Mask[Capacity];
    public fixed int Comparison[Capacity];
    public fixed int Bytes[Capacity];
    public fixed int Segment[Capacity];
    public fixed ulong Written[WriteCapacity];

    public static ListMemoryWaits* Of(XboxObject* list)
    {
        return (ListMemoryWaits*)((byte*)list + Offset);
    }

    /// <summary>A new recording.</summary>
    [HotPath]
    public void Reset()
    {
        Count = 0;
        Dropped = 0;
        Writes = 0;
        WorkAtSplit = 0;
    }

    /// <summary>A bottom-of-pipe write to <paramref name="address"/> was recorded.</summary>
    [HotPath]
    public void Wrote(ulong address)
    {
        Written[Writes % WriteCapacity] = address;
        Writes++;
    }

    /// <summary>Whether the list wrote <paramref name="address"/> before (among the last <see cref="WriteCapacity"/> writes).</summary>
    [HotPath]
    public readonly bool HasWritten(ulong address)
    {
        int written = Writes;

        if (written > WriteCapacity)
        {
            written = WriteCapacity;
        }

        for (int i = 0; i < written; i++)
        {
            if (Written[i] == address)
            {
                return true;
            }
        }

        return false;
    }

    /// <summary>Keeps a wait before segment <paramref name="segment"/>; false when no room is left.</summary>
    [HotPath]
    public bool Record(ulong address, int bytes, int comparison, ulong value, ulong mask, int segment)
    {
        if (Count == Capacity)
        {
            Dropped++;

            return false;
        }

        Address[Count] = address;
        Bytes[Count] = bytes;
        Comparison[Count] = comparison;
        Value[Count] = value;
        Mask[Count] = mask;

        Segment[Count] = segment;
        Count++;

        return true;
    }
}
