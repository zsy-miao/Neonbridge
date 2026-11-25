// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Numerics;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using FiveStar.Formats.Tests.Support;
using FiveStar.Formats.Ttpf.BcPack;
using Xunit;

namespace FiveStar.Formats.Tests;

/// <summary>
/// The BCPack BC1 decoder: its tables, the vectorised inverse against the hardware's, the synthetic streams of
/// bcpack-rs (vendor/bcpack-rs/tests/vectors) on every engine, and the engines against each other on damaged streams.
/// </summary>
public sealed class Bc1DecoderTests
{
    // SHA-256 of the five model tables, one line of comma-separated values each (bcpack-rs TABLES_SHA256).
    private const string ModelTablesSha256 = "346464df83c8be00905672091f2c64ac2eb0bb02efa9074a78b7620956f362d7";

    private static readonly int[] FieldWidths = [5, 6, 5, 5, 6, 5];

    [Fact]
    public void The_embedded_tables_are_the_hardware_models_and_the_corrected_window()
    {
        var table = RansModels.Table;
        var text = new StringBuilder();

        text.AppendJoin(',', table.Base).Append('\n');
        text.AppendJoin(',', table.Freq).Append('\n');
        text.AppendJoin(',', table.Rf).Append('\n');
        text.AppendJoin(',', table.Rs).Append('\n');
        text.AppendJoin(',', table.Rn).Append('\n');

        var hash = Convert.ToHexStringLower(SHA256.HashData(Encoding.ASCII.GetBytes(text.ToString())));
        var unfitted = Bc1Codebook.Default.Unfitted.Count(flag => flag != 0);

        Assert.Equal(ModelTablesSha256, hash);
        Assert.Equal(1, unfitted);
        Assert.Equal(1, Bc1Codebook.Default.Unfitted[0]);
    }

    [Fact]
    public void The_fast_inverse_equals_the_hardware_inverse_for_every_model_and_input()
    {
        for (var alphabet = 0; alphabet < MduModelTable.AlphabetIndices; alphabet++)
        {
            for (var q = 0; q < MduModelTable.ModelsPerAlphabet; q++)
            {
                var segmentBase = (uint)(RansModels.Index(alphabet, q) * RansModels.SegmentStride);

                for (uint cf = 0; cf <= 0xFFFF; cf++)
                {
                    var symbol = RansModels.Inverse(segmentBase, cf);
                    var segment = RansModels.Segment(segmentBase, cf);
                    var (freq, cum) = RansModels.ForwardExact(alphabet, q, symbol);

                    // The AVX-512 engine takes the symbol's forward constants from the inverse's segment.
                    var inSegment = 32 - BitOperations.LeadingZeroCount(symbol) == segment;
                    var inInterval = cf == 0 || (cf >= cum && cf < cum + freq);

                    if (symbol != RansModels.InverseExact(alphabet, q, cf) || symbol >> (alphabet + 1) != 0 || !inSegment || !inInterval)
                    {
                        Assert.Fail($"alphabet {alphabet} q {q} cf {cf}: symbol {symbol}, segment {segment}, interval [{cum}, {cum + freq})");
                    }
                }
            }
        }
    }

    [EngineFact(BcPackEngine.Scalar)]
    public void Synthetic_streams_decode_to_their_recorded_symbols_and_blocks_on_the_scalar_engine()
    {
        DecodeSyntheticStreams(BcPackEngine.Scalar);
    }

    [EngineFact(BcPackEngine.Avx2)]
    public void Synthetic_streams_decode_to_their_recorded_symbols_and_blocks_on_the_avx2_engine()
    {
        DecodeSyntheticStreams(BcPackEngine.Avx2);
    }

    [EngineFact(BcPackEngine.Avx512)]
    public void Synthetic_streams_decode_to_their_recorded_symbols_and_blocks_on_the_avx512_engine()
    {
        DecodeSyntheticStreams(BcPackEngine.Avx512);
    }

    [EngineFact(BcPackEngine.Avx2)]
    public void The_avx2_engine_matches_the_scalar_engine_on_damaged_streams()
    {
        DecodeDamagedStreams(BcPackEngine.Avx2);
    }

    [EngineFact(BcPackEngine.Avx512)]
    public void The_avx512_engine_matches_the_scalar_engine_on_damaged_streams()
    {
        DecodeDamagedStreams(BcPackEngine.Avx512);
    }

    private static void DecodeSyntheticStreams(BcPackEngine engine)
    {
        foreach (var vector in Vectors())
        {
            var symbols = new ushort[Bc1Decoder.Lanes * vector.Blocks];
            var output = new byte[vector.Blocks * Bc1Decoder.BlockSize];

            var decoded = BcPackDecoder.DecodeSymbols(vector.Chunk, BcPackCodec.Bc1, vector.Blocks, symbols, engine);
            var blocks = Bc1Decoder.Decode(vector.Chunk, vector.Blocks, output, TestCodebook(), engine);
            var cut = Bc1Decoder.Decode(vector.Chunk.AsSpan(0, vector.Chunk.Length / 2), vector.Blocks, new byte[output.Length], engine);

            Assert.True(decoded.Success, decoded.Describe());
            Assert.Equal(vector.Symbols, symbols);
            Assert.True(blocks.Success, blocks.Describe());
            Assert.Equal(vector.Bc1Sha256, Convert.ToHexStringLower(SHA256.HashData(output)));
            Assert.Equal(BcPackDecodeStatus.NoProgress, cut.Status);
        }
    }

    // A damaged stream fails the same way on every engine, and a stream that still converges gives the same blocks.
    private static void DecodeDamagedStreams(BcPackEngine engine)
    {
        var random = new Random(1);

        foreach (var vector in Vectors())
        {
            for (var trial = 0; trial < 300; trial++)
            {
                var damaged = (byte[])vector.Chunk.Clone();
                damaged[random.Next(damaged.Length)] ^= (byte)random.Next(1, 256);
                var scalarOutput = new byte[vector.Blocks * Bc1Decoder.BlockSize];
                var vectorOutput = new byte[vector.Blocks * Bc1Decoder.BlockSize];

                var scalar = Bc1Decoder.Decode(damaged, vector.Blocks, scalarOutput, BcPackEngine.Scalar);
                var vectorised = Bc1Decoder.Decode(damaged, vector.Blocks, vectorOutput, engine);

                Assert.Equal(scalar, vectorised);
                Assert.True(!scalar.Success || scalarOutput.AsSpan().SequenceEqual(vectorOutput));
            }
        }
    }

    /// <summary>The test tables of bcpack-rs tests/roundtrip.rs, which the vectors' block hashes use.</summary>
    private static Bc1Codebook TestCodebook()
    {
        var window = new byte[256 * 6];
        var codes = new byte[256 * 4];

        for (var slot = 0; slot < 256; slot++)
        {
            for (var field = 0; field < 6; field++)
            {
                window[(slot * 6) + field] = (byte)(((slot * 7) + (field * 13)) % (1 << FieldWidths[field]));
            }

            for (var code = 0; code < 4; code++)
            {
                codes[(slot * 4) + code] = (byte)((slot >> (2 * code)) & 3);
            }
        }

        return Bc1Codebook.Create(window, codes);
    }

    private static List<SyntheticStream> Vectors()
    {
        var files = Directory.GetFiles(Path.Combine(AppContext.BaseDirectory, "BcPackVectors"), "*.json");
        var vectors = new List<SyntheticStream>();
        Array.Sort(files, StringComparer.Ordinal);

        foreach (var file in files)
        {
            using var document = JsonDocument.Parse(File.ReadAllText(file));
            var root = document.RootElement;
            var symbols = new List<ushort>();

            foreach (var lane in root.GetProperty("symbols").EnumerateArray())
            {
                symbols.AddRange(lane.EnumerateArray().Select(symbol => (ushort)symbol.GetInt32()));
            }

            var chunk = Convert.FromHexString(root.GetProperty("chunk_hex").GetString()!);
            vectors.Add(new SyntheticStream(root.GetProperty("blocks").GetInt32(), chunk, symbols.ToArray(), root.GetProperty("bc1_sha256").GetString()!));
        }

        Assert.NotEmpty(vectors);

        return vectors;
    }

    private sealed record SyntheticStream(int Blocks, byte[] Chunk, ushort[] Symbols, string Bc1Sha256);
}
