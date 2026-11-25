// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Text.Json;
using BenchmarkDotNet.Attributes;
using FiveStar.Formats.Ttpf.BcPack;

namespace FiveStar.Benchmarks;

/// <summary>
/// The BCPack BC1 decoder on the 700-block synthetic stream of bcpack-rs (vendor/bcpack-rs), once per lane engine the
/// processor runs. <c>fivestar texture bcpack-bench</c> measures the same decoder on real pages of a dump.
/// BenchmarkDotNet derives a runner from the class, so it cannot be sealed.
/// </summary>
[MemoryDiagnoser]
public class Bc1DecodeBenchmarks
{
    private const string StreamResource = "FiveStar.Benchmarks.Bc1Stream.json";

    private byte[] _chunk = [];
    private byte[] _output = [];
    private int _blocks;

    [ParamsSource(nameof(Engines))]
    public BcPackEngine Engine { get; set; }

    public static IEnumerable<BcPackEngine> Engines()
    {
        return Enum.GetValues<BcPackEngine>().Where(BcPackDecoder.IsSupported);
    }

    [GlobalSetup]
    public void Setup()
    {
        using var stream = typeof(Bc1DecodeBenchmarks).Assembly.GetManifestResourceStream(StreamResource)
            ?? throw new InvalidOperationException("the synthetic stream is not embedded");
        using var document = JsonDocument.Parse(stream);

        _blocks = document.RootElement.GetProperty("blocks").GetInt32();
        _chunk = Convert.FromHexString(document.RootElement.GetProperty("chunk_hex").GetString()!);
        _output = new byte[_blocks * Bc1Decoder.BlockSize];
    }

    /// <summary>One decode of the stream to BC1 blocks.</summary>
    [Benchmark]
    public BcPackDecodeStatus Decode()
    {
        return Bc1Decoder.Decode(_chunk, _blocks, _output, Engine).Status;
    }
}
