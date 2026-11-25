// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Collections.Concurrent;

namespace FiveStar.Loader.Diagnostics;

/// <summary>
/// The catch of every [UnmanagedCallersOnly] entry point (an architecture rule checks that each has one). An exception
/// that leaves such a method crosses into the game's native frames as SEH exception 0xE0434352 and ends the process
/// without a managed stack (a KeyNotFoundException out of ExecuteIndirect did, in boot 17 and present's t3). Here it is
/// logged with its stack, the first time per entry point, then counted; the entry point returns its failure value.
/// </summary>
internal static class UnmanagedGuard
{
    private static readonly ConcurrentDictionary<string, long> Counts = new(StringComparer.Ordinal);
    private static Action<string>? _log;

    /// <summary>Exceptions caught so far, over every entry point.</summary>
    public static long Total => Counts.Values.Sum();

    /// <summary>Where reports go; until set, to the standard error stream.</summary>
    public static void LogTo(Action<string> log)
    {
        _log = log;
    }

    /// <summary>An exception caught at entry point <paramref name="entry"/> (type and method).</summary>
    public static void Caught(Exception exception, string entry)
    {
        try
        {
            long count = Counts.AddOrUpdate(entry, 1, (_, previous) => previous + 1);

            if (count == 1)
            {
                Report($"[guard] {entry} threw; it returns its failure value from now on when it does: {exception}");
            }
            else if ((count & (count - 1)) == 0)
            {
                Report($"[guard] {entry} threw {count} times ({exception.GetType().Name}: {exception.Message})");
            }
        }
        catch (Exception)
        {
            // Reporting must never throw out of an entry point either.
        }
    }

    private static void Report(string message)
    {
        var log = Volatile.Read(ref _log);

        if (log is null)
        {
            Console.Error.WriteLine(message);

            return;
        }

        log(message);
    }
}
