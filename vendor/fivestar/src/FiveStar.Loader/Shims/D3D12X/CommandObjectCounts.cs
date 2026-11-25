// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X;

/// <summary>
/// Desktop command allocators and lists made so far, the game's and the loader's (spares lent at a busy allocator,
/// lists a split at a memory wait records on), for the sample summary: an allocator keeps the memory of its largest
/// recording, so they are candidates for the write-combined memory that grows from the story load.
/// </summary>
internal static class CommandObjectCounts
{
    private static long _allocators;
    private static long _spares;
    private static long _sparesReleased;
    private static long _lists;
    private static long _splitLists;

    public static void AllocatorMade()
    {
        Interlocked.Increment(ref _allocators);
    }

    public static void SpareMade()
    {
        Interlocked.Increment(ref _spares);
    }

    public static void SparesReleased(int count)
    {
        Interlocked.Add(ref _sparesReleased, count);
    }

    public static void ListMade()
    {
        Interlocked.Increment(ref _lists);
    }

    public static void SplitListMade()
    {
        Interlocked.Increment(ref _splitLists);
    }

    /// <summary>"command objects: A allocators (+S spares, R of them released idle), L lists (+C for splits, K splits)".</summary>
    public static string Summary => $"command objects: {Interlocked.Read(ref _allocators)} allocators (+{Interlocked.Read(ref _spares)} spares, {Interlocked.Read(ref _sparesReleased)} of them released idle), {Interlocked.Read(ref _lists)} lists (+{Interlocked.Read(ref _splitLists)} for splits, {ListSplits.Splits} splits)";
}
