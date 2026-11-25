// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Diagnostics;

/// <summary>The process heaps (GetProcessHeaps) and what each commits and holds allocated (HeapSummary).</summary>
internal static unsafe class HeapUsage
{
    private const double BytesPerGigabyte = 1L << 30;

    public static nint[] Handles(int max)
    {
        var heaps = new nint[max];
        uint count;

        fixed (nint* first = heaps)
        {
            count = Kernel32.GetProcessHeaps((uint)max, first);
        }

        return heaps[..(int)Math.Min(count, (uint)max)];
    }

    /// <summary>The count and totals of the heaps, and the <paramref name="top"/> committing the most.</summary>
    public static string Summary(IEnumerable<nint> heaps, int top)
    {
        var summaries = new List<(nint Heap, long Committed, long Allocated)>();

        foreach (nint heap in heaps)
        {
            HeapSummaryInfo summary = default;
            summary.Size = (uint)sizeof(HeapSummaryInfo);

            if (Kernel32.HeapSummary(heap, 0, &summary) != 0)
            {
                summaries.Add((heap, (long)summary.Committed, (long)summary.Allocated));
            }
        }

        var largest = summaries
            .OrderByDescending(s => s.Committed)
            .Take(top)
            .Select(s => $"0x{s.Heap:x} {Gigabytes(s.Committed)} ({Gigabytes(s.Allocated)} allocated)");

        return $"{summaries.Count} heaps commit {Gigabytes(summaries.Sum(s => s.Committed))} ({Gigabytes(summaries.Sum(s => s.Allocated))} allocated); largest {string.Join(", ", largest)}";
    }

    private static string Gigabytes(long bytes)
    {
        return string.Create(CultureInfo.InvariantCulture, $"{bytes / BytesPerGigabyte:F2} GB");
    }
}
