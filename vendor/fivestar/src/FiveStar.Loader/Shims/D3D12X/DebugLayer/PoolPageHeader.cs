// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Shims.D3D12X.DebugLayer;

/// <summary>
/// The header of a page of the game's small-block pools (free 0x14540abb0, allocate 0x144dac530): 16 KiB pages, the
/// header at the page's start. Its first qword is the free list: the low dword the offset of the first free block in
/// the page (each free block's first dword the next one's), bits 32 to 47 a block count, bits 48 to 63 a counter every
/// compare-exchange adds one to. +8 and +16 link the page into its pool's lists, the byte at +34 says whether it is on
/// the partial list. The CPU changes the first qword only by such a compare-exchange: the counter moves forward by the
/// operations done, the offset stays inside the page and the list stays inside it.
/// </summary>
[StructLayout(LayoutKind.Sequential)]
internal readonly record struct PoolPageHeader(ulong FreeList, ulong Next, ulong Previous, byte Partial, uint FirstNext)
{
    public const int PageBytes = 0x4000;

    /// <summary>The partial-list flag's offset in the header.</summary>
    public const int PartialOffset = 34;

    /// <summary>Header bytes before the first block.</summary>
    public const int HeaderBytes = 40;

    /// <summary>Blocks a page holds at most (8-byte blocks: boot 16 saw counts over 1,024).</summary>
    private const uint MaxBlocks = PageBytes / 8;

    /// <summary>Operations one page can see between two checks before a counter step looks like garbage.</summary>
    private const int MaxSteps = 4096;

    private const int CountShift = 32;
    private const int CounterShift = 48;
    private const ulong CountMask = 0xFFFF;

    /// <summary>The highest canonical user-mode address.</summary>
    private const ulong UserEnd = 0x0000_7FFF_FFFF_FFFF;

    public uint FirstFree => (uint)FreeList;

    public uint Count => (uint)((FreeList >> CountShift) & CountMask);

    public ushort Counter => (ushort)(FreeList >> CounterShift);

    /// <summary>Whether this reads as a pool page header: offsets inside the page, a block count, list links and the flag.</summary>
    public bool Plausible()
    {
        bool links = Link(Next) && Link(Previous) && Next != 0 && Previous != 0;
        bool free = FirstFree == 0 || (FirstFree >= HeaderBytes && FirstFree < PageBytes);

        return links && free && Count <= MaxBlocks && Partial <= 1 && FreeListContinues();
    }

    /// <summary>
    /// Whether <paramref name="later"/> can be this header after CPU allocations and frees: unchanged, or plausible with
    /// its counter moved forward by 1 to <see cref="MaxSteps"/>.
    /// </summary>
    public bool CpuCanBecome(PoolPageHeader later)
    {
        if (later.FreeList == FreeList)
        {
            return later.Plausible() || later == this;
        }

        int steps = (ushort)(later.Counter - Counter);

        return later.Plausible() && steps >= 1 && steps <= MaxSteps;
    }

    /// <summary>
    /// Whether <paramref name="later"/> is the allocator still at work on this page (its counter moved forward by 1 to
    /// <see cref="MaxSteps"/>); a change that is not is the page given back and its memory used for something else.
    /// </summary>
    public bool StillPooled(PoolPageHeader later)
    {
        int steps = (ushort)(later.Counter - Counter);

        return steps >= 1 && steps <= MaxSteps;
    }

    /// <summary>The free list's second offset must lie in the page too (0 ends it).</summary>
    private bool FreeListContinues()
    {
        return FirstFree == 0 || FirstNext == 0 || FirstNext < PageBytes;
    }

    private static bool Link(ulong address)
    {
        return address <= UserEnd && address % sizeof(ulong) == 0;
    }
}
