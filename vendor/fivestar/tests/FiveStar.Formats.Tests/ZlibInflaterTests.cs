// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Formats.Compression;
using FiveStar.Formats.Tests.Support;
using Xunit;

namespace FiveStar.Formats.Tests;

/// <summary>The inflater on streams from the runtime's compressor.</summary>
public sealed class ZlibInflaterTests
{
    [Fact]
    public void A_zlib_stream_inflates_and_reports_its_exact_length()
    {
        var data = Deflate.Pattern(100_000);
        var stream = Deflate.Zlib(data);
        var input = stream.Concat(Deflate.Noise(64, 7)).ToArray();
        var inflater = new ZlibInflater();

        var result = inflater.Inflate(input, DeflateWrapper.Zlib, int.MaxValue);

        Assert.Equal(InflateStatus.Complete, result.Status);
        Assert.Equal(stream.Length, result.Consumed);
        Assert.Equal(data, inflater.Output.ToArray());
    }

    [Fact]
    public void A_corrupt_cut_or_oversized_stream_is_refused()
    {
        var data = Deflate.Pattern(20_000);
        var stream = Deflate.Zlib(data);
        var corrupt = (byte[])stream.Clone();
        corrupt[^1] ^= 0xFF;
        var inflater = new ZlibInflater();

        var checksum = inflater.Inflate(corrupt, DeflateWrapper.Zlib, int.MaxValue).Status;
        var cut = inflater.Inflate(stream.AsSpan(0, stream.Length / 2), DeflateWrapper.Zlib, int.MaxValue).Status;
        var limited = inflater.Inflate(stream, DeflateWrapper.Zlib, 1000).Status;

        Assert.Equal(InflateStatus.Invalid, checksum);
        Assert.Equal(InflateStatus.Truncated, cut);
        Assert.Equal(InflateStatus.OutputLimit, limited);
    }
}
