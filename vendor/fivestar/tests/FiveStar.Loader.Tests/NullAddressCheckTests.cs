// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Shims.D3D12X.DebugLayer;
using Xunit;

namespace FiveStar.Loader.Tests;

/// <summary>--check-null-addresses: which addresses and ranges it drops.</summary>
public sealed class NullAddressCheckTests
{
    [Fact]
    public void Null_and_low_addresses_and_reads_past_the_buffer_are_dropped()
    {
        Assert.True(NullAddressCheck.Suspect(0));
        Assert.True(NullAddressCheck.Suspect(0x40));
        Assert.False(NullAddressCheck.Suspect(0x2_0000_0000));

        Assert.False(NullAddressCheck.PastEnd(0x100, 0x100, 0x200));
        Assert.True(NullAddressCheck.PastEnd(0x100, 0x101, 0x200));
        Assert.True(NullAddressCheck.PastEnd(0x300, 0, 0x200));
        Assert.True(NullAddressCheck.PastEnd(0x10, ulong.MaxValue, 0x200));

        Assert.Equal("no desktop argument buffer", NullAddressCheck.Indirect(0, 0, 12, 0, 0));
        Assert.Equal("buffer copy of 0x40 bytes with no desktop source", NullAddressCheck.Copy(1, 0, 0, 0, 0x40));
    }
}
