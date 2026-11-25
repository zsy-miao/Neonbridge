// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Diagnostics;
using Xunit;

namespace FiveStar.Loader.Tests;

/// <summary>An exception caught at an [UnmanagedCallersOnly] boundary is logged with its stack once, then counted.</summary>
public sealed class UnmanagedGuardTests
{
    [Fact]
    public void The_first_exception_per_entry_point_is_logged_in_full_and_later_ones_at_powers_of_two()
    {
        var lines = new List<string>();
        UnmanagedGuard.LogTo(line =>
        {
            lock (lines)
            {
                lines.Add(line);
            }
        });

        const string Entry = "UnmanagedGuardTests.Entry";

        for (int i = 0; i < 5; i++)
        {
            UnmanagedGuard.Caught(Thrown(), Entry);
        }

        var mine = lines.Where(line => line.Contains(Entry, StringComparison.Ordinal)).ToList();
        Assert.Equal(3, mine.Count);
        Assert.Contains("KeyNotFoundException", mine[0], StringComparison.Ordinal);
        Assert.Contains(nameof(Thrown), mine[0], StringComparison.Ordinal);
        Assert.Contains("threw 4 times", mine[2], StringComparison.Ordinal);
    }

    private static KeyNotFoundException Thrown()
    {
        try
        {
            throw new KeyNotFoundException("'.link-0' is not in the index");
        }
        catch (KeyNotFoundException exception)
        {
            return exception;
        }
    }
}
