// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using System.Text.RegularExpressions;
using FiveStar.Loader.Shims.D3D12X.DebugLayer;
using FiveStar.Shaders.Translation;
using Xunit;

namespace FiveStar.Loader.Tests;

/// <summary>The store trace's ring: the prelude writes it where the loader reads it.</summary>
public sealed partial class PoolStoreTraceTests
{
    [Fact]
    public void The_prelude_ring_starts_where_the_loader_reads_it()
    {
        var prelude = RuntimeIncludes.Read("rdna_prelude.hlsli");
        var match = TraceBase().Match(prelude);

        Assert.True(match.Success);
        Assert.Equal(PoolStoreTrace.TraceBase, uint.Parse(match.Groups[1].Value, NumberStyles.HexNumber, CultureInfo.InvariantCulture));
    }

    [GeneratedRegex(@"TRACE_BASE = 0x([0-9a-f]+)u")]
    private static partial Regex TraceBase();
}
