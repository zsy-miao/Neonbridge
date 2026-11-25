// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Collections.Concurrent;
using System.Diagnostics;

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>
/// The pipeline creations running now, one per thread: the slot, what is being created and since when. The game waits
/// for its deferred creation jobs before it finishes a frame, so a creation the driver never returns from stops the
/// frame; the sample summary names it.
/// </summary>
internal sealed class PipelineCreationsInFlight
{
    private readonly ConcurrentDictionary<int, PipelineCreation> _running = new();

    /// <summary>For <see cref="Begin"/>: a creation not made through a device slot (a build thread's driver compile).</summary>
    public const int NoSlot = -1;

    /// <summary>Marks the calling thread as creating a pipeline through <paramref name="slot"/>.</summary>
    public void Begin(int slot)
    {
        _running[Environment.CurrentManagedThreadId] = new PipelineCreation(slot, Stopwatch.GetTimestamp(), "description");
    }

    /// <summary>Names what the calling thread's creation is building (a program name, a shader count).</summary>
    public void Describe(string what)
    {
        if (_running.TryGetValue(Environment.CurrentManagedThreadId, out var entry))
        {
            _running[Environment.CurrentManagedThreadId] = entry with { What = what };
        }
    }

    /// <summary>What the calling thread's creation was last described as; empty outside a creation.</summary>
    public string Current()
    {
        if (_running.TryGetValue(Environment.CurrentManagedThreadId, out var entry))
        {
            return entry.What;
        }

        return string.Empty;
    }

    /// <summary>Ends the calling thread's creation.</summary>
    public void End()
    {
        _running.TryRemove(Environment.CurrentManagedThreadId, out _);
    }

    /// <summary>The creations running now, oldest first, with their age; empty when none.</summary>
    public string Summary()
    {
        var entries = _running.Values.OrderBy(entry => entry.Started).ToArray();

        if (entries.Length == 0)
        {
            return string.Empty;
        }

        var text = entries.Select(entry => $"{SlotText(entry.Slot)}{entry.What} {Stopwatch.GetElapsedTime(entry.Started).TotalSeconds:F0} s");

        return $"{entries.Length} in flight ({string.Join(", ", text)})";
    }

    private static string SlotText(int slot)
    {
        if (slot == NoSlot)
        {
            return string.Empty;
        }

        return $"slot {slot} ";
    }
}
