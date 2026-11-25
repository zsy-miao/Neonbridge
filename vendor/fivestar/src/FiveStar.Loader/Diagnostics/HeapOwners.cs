// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Collections.Concurrent;
using System.Globalization;
using FiveStar.Loader.Loading;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Diagnostics;

/// <summary>
/// Which heap each heap segment of a memory map belongs to, and who made that heap: the heaps a mapped image creates
/// through <see cref="Shims.HeapShims"/> keep the game frame of the creating call; the process heap (every CRT's malloc,
/// the game's shipped one and the loader's NativeMemory alike) is named as such; any other was made by a system or
/// driver DLL. A boot on a 15.6 GB machine held 9.02 GB in 601 heap allocations after 18 minutes (issue #13), with
/// nothing to tell whose.
/// </summary>
internal static unsafe class HeapOwners
{
    /// <summary>Return addresses searched for a frame of a mapped image.</summary>
    private const int Frames = 16;

    /// <summary>HEAP_SEGMENT.Heap: the heap an NT heap segment belongs to (+0x28 on x64).</summary>
    private const int SegmentHeapOffset = 0x28;

    /// <summary>The NT heap segment signature at +0x10 (HEAP_SEGMENT.SegmentSignature).</summary>
    private const uint NtSegmentSignature = 0xFFEEFFEE;

    private const double BytesPerGigabyte = 1L << 30;

    private static readonly ConcurrentDictionary<nint, nint> Creators = new();

    /// <summary>A heap created by mapped code: the first return address in a mapped image (0 for none).</summary>
    public static void Created(nint heap, ImageTable images)
    {
        if (heap == 0)
        {
            return;
        }

        nint* frames = stackalloc nint[Frames];
        int count = Kernel32.RtlCaptureStackBackTrace(0, Frames, frames, null);
        nint creator = 0;

        for (int i = 0; i < count && creator == 0; i++)
        {
            if (images.FindByAddress(frames[i]) is not null)
            {
                creator = frames[i];
            }
        }

        Creators[heap] = creator;
    }

    public static void Destroyed(nint heap)
    {
        Creators.TryRemove(heap, out _);
    }

    /// <summary>
    /// The heap an allocation labeled heap belongs to: itself when it is a heap's first segment (its handle), else the
    /// heap an NT segment names at +0x28; 0 when it cannot be read.
    /// </summary>
    public static nint Owner(nint allocation, uint signature, HashSet<nint> heaps)
    {
        if (heaps.Contains(allocation) || signature != NtSegmentSignature)
        {
            return allocation;
        }

        nint owner = 0;
        nuint read = 0;
        _ = Kernel32.ReadProcessMemory(Kernel32.GetCurrentProcess(), allocation + SegmentHeapOffset, &owner, (nuint)sizeof(nint), &read);

        return owner;
    }

    /// <summary>Who made <paramref name="heap"/>: the process heap, the game frame of its creation, or a DLL of the system.</summary>
    public static string Describe(nint heap, ImageTable images)
    {
        if (heap == Kernel32.GetProcessHeap())
        {
            return "the process heap (malloc of every CRT, the loader's native memory)";
        }

        if (Creators.TryGetValue(heap, out nint creator) && creator != 0)
        {
            return $"made at {images.Describe(creator)}";
        }

        if (Creators.ContainsKey(heap))
        {
            return "made through the shims outside the game's images";
        }

        return "made by a system or driver DLL";
    }

    /// <summary>"heap segments by heap: 0x... (who) 1.23 GB in 45, ..." for the <paramref name="top"/> heaps holding the most.</summary>
    public static string Summary(IEnumerable<(nint Owner, long Bytes)> segments, ImageTable images, int top)
    {
        var largest = segments
            .GroupBy(segment => segment.Owner)
            .Select(group => (Heap: group.Key, Bytes: group.Sum(segment => segment.Bytes), Count: group.Count()))
            .OrderByDescending(group => group.Bytes)
            .Take(top)
            .Select(group => string.Create(CultureInfo.InvariantCulture, $"0x{group.Heap:x} ({Describe(group.Heap, images)}) {group.Bytes / BytesPerGigabyte:F2} GB in {group.Count}"));

        return $"heap segments by heap: {string.Join(", ", largest)}";
    }
}
