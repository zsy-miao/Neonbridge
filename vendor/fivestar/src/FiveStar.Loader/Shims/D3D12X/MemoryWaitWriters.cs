// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Collections.Concurrent;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.D3D12X;

/// <summary>
/// The last list that recorded a bottom-of-pipe write to each address, for the memory-wait reports
/// (<see cref="MemoryWaits"/>): whether that writer was submitted after it recorded the write, to which queue, and
/// whether that queue has run it.
/// </summary>
internal static unsafe class MemoryWaitWriters
{
    /// <summary>ID3D12Fence::GetCompletedValue (d3d12.h).</summary>
    private const int GetCompletedValueSlot = 8;

    private static readonly ConcurrentDictionary<ulong, (nint List, long At, ulong Value)> Writers = new();

    /// <summary>A bottom-of-pipe write of <paramref name="value"/> to <paramref name="address"/> recorded in <paramref name="list"/>.</summary>
    [HotPath]
    public static void Wrote(XboxObject* list, ulong address, ulong value)
    {
        ListMemoryWaits.Of(list)->Wrote(address);
        Writers[address] = ((nint)list, Environment.TickCount64, value);
    }

    /// <summary><paramref name="list"/> (a wrapper) was submitted to <paramref name="queue"/>.</summary>
    [HotPath]
    public static void Submitted(nint list, XboxQueueObject* queue)
    {
        var waits = ListMemoryWaits.Of((XboxObject*)list);
        waits->SubmittedAt = Environment.TickCount64;
        waits->Queue = queue->Number;
        MemoryWaits.WriterSubmitted(list, queue->Number);
    }

    /// <summary>Who writes <paramref name="address"/>, as far as the lists show.</summary>
    public static string Describe(ulong address)
    {
        if (!Writers.TryGetValue(address, out var writer))
        {
            return "no list recorded a write there";
        }

        var waits = ListMemoryWaits.Of((XboxObject*)writer.List);
        long now = Environment.TickCount64;
        string recorded = $"list 0x{writer.List:x} recorded 0x{writer.Value:x} there {now - writer.At} ms ago";

        if (waits->SubmittedAt < writer.At)
        {
            return $"{recorded} and has not been submitted since";
        }

        var submission = ListSubmission.Of(writer.List);
        ulong completed = 0;

        if (submission->Fence != 0)
        {
            completed = ((delegate* unmanaged<nint, ulong>)ComObject.Slot(submission->Fence, GetCompletedValueSlot))(submission->Fence);
        }

        return $"{recorded}, submitted to queue #{waits->Queue} {now - waits->SubmittedAt} ms ago (fence {completed} of {submission->Value})";
    }
}
