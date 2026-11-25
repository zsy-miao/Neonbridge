// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using System.Text;

namespace FiveStar.Loader.Shims.D3D12X;

/// <summary>
/// The work a game thread waiting on a fence waits for: the queue that was asked to signal the fence to the awaited
/// value, and the executions it was given before that Signal, newest first, with their last pipelines, whether they
/// finished, and any Wait on another fence among them. A stall report then names the pass the GPU did not finish
/// (the game's fence wait ends in an assertion or its hang watchdog's write to address 0, issue #13).
/// </summary>
internal static class StallCulprit
{
    /// <summary>Executions named before the Signal.</summary>
    public const int Shown = 6;

    /// <summary>
    /// Where <paramref name="events"/> (one queue's kept calls, oldest first) signal <paramref name="fence"/> to
    /// <paramref name="target"/> or past it, the first such Signal and the work before it; null when they do not.
    /// <paramref name="completed"/> is the queue's finished execution value; <paramref name="name"/> names a pipeline.
    /// </summary>
    public static string? Describe(int queue, IReadOnlyList<QueueEvent> events, nint fence, ulong target, ulong completed, Func<nint, string> name)
    {
        int signal = -1;

        for (int i = 0; i < events.Count; i++)
        {
            var entry = events[i];

            if (entry.Kind == QueueEventKind.Signal && entry.Fence == fence && entry.Value >= target)
            {
                signal = i;

                break;
            }
        }

        if (signal < 0)
        {
            return null;
        }

        var text = new StringBuilder();
        text.Append(CultureInfo.InvariantCulture, $"queue #{queue} signals it to {events[signal].Value} after");
        int shown = 0;

        for (int i = signal - 1; i >= 0 && shown < Shown; i--)
        {
            var entry = events[i];

            if (entry.Kind == QueueEventKind.Execute)
            {
                string state = entry.Value <= completed ? "done" : "NOT DONE";
                text.Append(CultureInfo.InvariantCulture, $"\n        execute #{entry.Value} {state}: {entry.Draws} draws, {entry.Dispatches} dispatches, {entry.Indirect} indirect; last pipeline {name(entry.Pipeline)}");
                shown++;
            }
            else if (entry.Kind == QueueEventKind.Wait)
            {
                text.Append(CultureInfo.InvariantCulture, $"\n        after a Wait for fence 0x{entry.Fence:x} to {entry.Value}");
            }
        }

        if (shown == 0)
        {
            text.Append(" no execution kept");
        }

        return text.ToString();
    }

    /// <summary>The last Signal of <paramref name="fence"/> in <paramref name="events"/>: its value, or null.</summary>
    public static ulong? LastSignal(IReadOnlyList<QueueEvent> events, nint fence)
    {
        for (int i = events.Count - 1; i >= 0; i--)
        {
            if (events[i].Kind == QueueEventKind.Signal && events[i].Fence == fence)
            {
                return events[i].Value;
            }
        }

        return null;
    }
}
