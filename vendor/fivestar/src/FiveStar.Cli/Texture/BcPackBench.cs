// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Diagnostics;
using System.Globalization;
using System.Numerics;
using FiveStar.Cli.Archive;
using FiveStar.Formats.Common;
using FiveStar.Formats.Ttpf.BcPack;

namespace FiveStar.Cli.Texture;

/// <summary>
/// <c>fivestar texture bcpack-bench</c>: single-thread decode speed on BC1 pages sampled evenly over the given page
/// files, by size class (block counts from 2^k to 2^(k+1) - 1). Time is the process's processor time, so other load
/// on the machine counts only through shared caches and clocks.
/// </summary>
internal sealed class BcPackBench
{
    private const long DefaultPages = 400;
    private const double DefaultSeconds = 2;
    private const double WarmUpSeconds = 1;
    private const double BytesPerMegabyte = 1e6;
    private const int SizeWidth = 6;
    private const int PageWidth = 4;
    private const int TwoDecimals = 2;
    private const int ThreeDecimals = 3;

    private readonly TextWriter _output;

    public BcPackBench(TextWriter output)
    {
        _output = output;
    }

    public void Run(IReadOnlyList<string> paths, CommandArguments arguments)
    {
        var engine = BcPackPageSet.Engine(arguments.Option("--engine"));
        var seconds = DefaultSeconds;
        var secondsOption = arguments.Option("--seconds");

        if (secondsOption is not null)
        {
            seconds = double.Parse(secondsOption, CultureInfo.InvariantCulture);
        }

        var sample = Sample(BcPackPageSet.Files(paths), Math.Max(1, arguments.LongOption("--pages", DefaultPages)));
        var output = new byte[sample.Max(page => page.Blocks) * Bc1Decoder.BlockSize];
        _output.WriteLine($"{BcPackPageSet.EngineName(engine)} engine, {sample.Count} sampled BC1 pages, processor time, {DecimalText.Fixed(seconds, 1)} s per size class");

        // A first pass lets the runtime finish optimising the decoder.
        Measure(sample, output, engine, WarmUpSeconds);
        long allBlocks = 0;
        double allSeconds = 0;

        foreach (var sizeClass in sample.GroupBy(page => BitOperations.Log2((uint)page.Blocks)).OrderBy(group => group.Key))
        {
            var pages = sizeClass.ToList();
            var (blocks, cycles, elapsed) = Measure(pages, output, engine, seconds);
            allBlocks += blocks;
            allSeconds += elapsed;

            var low = 1 << sizeClass.Key;
            var rate = DecimalText.Fixed(blocks * Bc1Decoder.BlockSize / BytesPerMegabyte / elapsed, 1);
            var blockRate = DecimalText.Fixed(blocks / BytesPerMegabyte / elapsed, TwoDecimals);
            var clocks = DecimalText.Fixed(cycles / (double)blocks, ThreeDecimals);
            _output.WriteLine($"  blocks {low,SizeWidth}-{(low << 1) - 1,-SizeWidth} ({pages.Count,PageWidth} pages): {rate} MB/s BC1 out, {blockRate} M blocks/s, {clocks} clocks/block");
        }

        _output.WriteLine($"  all classes: {DecimalText.Fixed(allBlocks * Bc1Decoder.BlockSize / BytesPerMegabyte / allSeconds, 1)} MB/s BC1 out per core");
    }

    /// <summary>Decodes the pages over and over for at least <paramref name="seconds"/> of processor time.</summary>
    private static (long Blocks, long Cycles, double Seconds) Measure(List<(int Blocks, byte[] Data)> pages, byte[] output, BcPackEngine engine, double seconds)
    {
        using var process = Process.GetCurrentProcess();
        var start = process.TotalProcessorTime;
        long blocks = 0;
        long cycles = 0;
        double elapsed;

        do
        {
            foreach (var (count, data) in pages)
            {
                var result = Bc1Decoder.Decode(data, count, output, engine);

                if (!result.Success)
                {
                    throw new InvalidDataException("a sampled page failed: " + result.Describe());
                }

                blocks += count;
                cycles += result.Cycles;
            }

            elapsed = (process.TotalProcessorTime - start).TotalSeconds;
        }
        while (elapsed < seconds);

        return (blocks, cycles, elapsed);
    }

    /// <summary>About <paramref name="count"/> decodable BC1 pages spread evenly over all of them, read into memory.</summary>
    private static List<(int Blocks, byte[] Data)> Sample(List<string> files, long count)
    {
        var pages = BcPackPageSet.Pages(files, 1, BcPackCodec.Bc1).Where(entry => entry.Page.Bc1Blocks > 0).ToList();
        var step = Math.Max(1, pages.Count / count);
        var sample = new List<(int Blocks, byte[] Data)>();
        FileByteSource? source = null;
        var open = -1;

        try
        {
            for (var index = 0; index < pages.Count; index += (int)step)
            {
                var (file, page) = pages[index];

                if (file != open)
                {
                    source?.Dispose();
                    source = new FileByteSource(files[file]);
                    open = file;
                }

                sample.Add((page.Bc1Blocks, source!.Read(page.FileOffset, (int)page.FileLength).ToArray()));
            }
        }
        finally
        {
            source?.Dispose();
        }

        return sample;
    }
}
