// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;

namespace FiveStar.Loader.Diagnostics;

/// <summary>
/// The process's commit by category, for the sample line: what its private bytes hold and what is charged outside them.
/// Write-combined memory is the driver's commit mirror of video memory (<see cref="WriteCombinedMemory"/>): the placed
/// texture heaps are part of it, the rest is other video memory (render targets, committed textures and buffers). What no
/// category names is the driver's own native memory: its compile arenas (the running compiles' estimate is shown beside
/// it), pipeline objects and pools. The XMem pages are pagefile-backed sections, charged once, not to private bytes.
/// </summary>
internal static class CommitAccounting
{
    private const double BytesPerGigabyte = 1L << 30;

    /// <summary>One process's figures, all bytes.</summary>
    public readonly record struct Figures(
        long Private,
        long WriteCombined,
        long PlacedTextureHeaps,
        long Managed,
        long GameMemory,
        long CompileArenasExpected,
        long XMemPages,
        long SectionHeaps,
        long Helpers);

    /// <summary>"commit by category: private P GB = ...; outside it ...".</summary>
    public static string Format(Figures figures)
    {
        long otherVideo = Math.Max(0, figures.WriteCombined - figures.PlacedTextureHeaps);
        long rest = figures.Private - figures.WriteCombined - figures.Managed - figures.GameMemory;

        return string.Create(
            CultureInfo.InvariantCulture,
            $"commit by category: private {Gb(figures.Private)} = video memory mirror {Gb(figures.WriteCombined)} (placed texture heaps {Gb(figures.PlacedTextureHeaps)}, other {Gb(otherVideo)}) + managed {Gb(figures.Managed)} + game memory {Gb(figures.GameMemory)} + driver and native {Gb(rest)} (compile arenas expected {Gb(figures.CompileArenasExpected)}); outside it XMem pages {Gb(figures.XMemPages)} (section heaps open over {Gb(figures.SectionHeaps)} of them), helper processes {Gb(figures.Helpers)}");
    }

    private static string Gb(long bytes)
    {
        return (bytes / BytesPerGigabyte).ToString("F1", CultureInfo.InvariantCulture) + " GB";
    }
}
