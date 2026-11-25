// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Diagnostics;
using System.Globalization;
using System.Runtime.InteropServices;
using System.Security.Cryptography;
using FiveStar.Cli.Archive;
using FiveStar.Formats.Common;
using FiveStar.Formats.Ttpf;
using FiveStar.Formats.Ttpf.BcPack;
using Microsoft.Win32.SafeHandles;

namespace FiveStar.Cli.Texture;

/// <summary>
/// <c>fivestar texture bcpack-scan</c>: decodes every BCPack page of one codec (<c>--codec</c>, BC1 by default) in the
/// given page files on all cores, then reports convergence per file and in total with the decode speed. <c>--hashes</c>
/// writes one line per page (path, virtual address, status, SHA-256 of the output, unfitted blocks) for comparing
/// engines or builds. A codec without a block decoder decodes to lane symbols, and the hash is of those.
/// </summary>
internal sealed class BcPackScan
{
    private const int FileNameWidth = 36;
    private const int CountWidth = 7;
    private const int FailedWidth = 5;
    private const int ListedFailures = 20;
    private const double BytesPerMegabyte = 1e6;
    private const double BytesPerGigabyte = 1e9;
    private const int TwoDecimals = 2;
    private const string NoHash = "-";

    private readonly TextWriter _output;

    public BcPackScan(TextWriter output)
    {
        _output = output;
    }

    /// <summary>Scans the pages; false when any page failed.</summary>
    public bool Run(IReadOnlyList<string> paths, CommandArguments arguments)
    {
        var files = BcPackPageSet.Files(paths);
        var engine = BcPackPageSet.Engine(arguments.Option("--engine"));
        var codec = BcPackPageSet.Codec(arguments.Option("--codec"));
        var jobs = (int)Math.Max(1, arguments.LongOption("--jobs", Environment.ProcessorCount));
        var pages = BcPackPageSet.Pages(files, Math.Max(1, arguments.LongOption("--every", 1)), codec);
        var hashes = arguments.Option("--hashes");

        var clock = Stopwatch.StartNew();
        var outcomes = DecodeAll(files, pages, new BcPackScanJob(codec, engine, hashes is not null), jobs);
        var wall = clock.Elapsed.TotalSeconds;

        var tallies = new BcPackScanTally[files.Count];
        var failures = new List<string>();
        long ticks = 0;

        for (var index = 0; index < files.Count; index++)
        {
            tallies[index] = new BcPackScanTally();
        }

        for (var index = 0; index < pages.Count; index++)
        {
            var (file, page) = pages[index];
            var result = outcomes[index].Result;
            tallies[file].Add(result, page.FileLength, page.VirtualSize);
            ticks += outcomes[index].Ticks;

            if (!result.Success)
            {
                failures.Add($"  {Path.GetFileName(files[file])} vaddr 0x{page.VirtualAddress:x} vsize {page.VirtualSize}: {Describe(codec, page, result)}");
            }
        }

        var total = PrintFiles(files, tallies, arguments.HasFlag("--quiet"));
        PrintFailures(failures);
        PrintTotals(new BcPackScanJob(codec, engine, false), files.Count, jobs, total, (wall, ticks / (double)Stopwatch.Frequency));

        if (hashes is not null)
        {
            File.WriteAllLines(hashes, HashLines(files, pages, outcomes));
        }

        return failures.Count == 0;
    }

    private static BcPackPageOutcome[] DecodeAll(List<string> files, List<(int File, TexturePage Page)> pages, BcPackScanJob job, int jobs)
    {
        var handles = new SafeFileHandle[files.Count];
        var outcomes = new BcPackPageOutcome[pages.Count];

        try
        {
            for (var index = 0; index < files.Count; index++)
            {
                handles[index] = File.OpenHandle(files[index], options: FileOptions.RandomAccess);
            }

            var parallel = new ParallelOptions { MaxDegreeOfParallelism = jobs };
            Parallel.For(0, pages.Count, parallel, () => new BcPackScanBuffers(), (index, loop, buffers) =>
            {
                outcomes[index] = DecodePage(handles[pages[index].File], pages[index].Page, job, buffers);

                return buffers;
            }, KeepBuffers);
        }
        finally
        {
            foreach (var handle in handles)
            {
                handle?.Dispose();
            }
        }

        return outcomes;
    }

    /// <summary>Reads one page's stored bytes and decodes them; the time is the decode's alone.</summary>
    private static BcPackPageOutcome DecodePage(SafeFileHandle file, TexturePage page, BcPackScanJob job, BcPackScanBuffers buffers)
    {
        var length = (int)page.FileLength;
        var count = BcPackDecoder.SymbolsPerLane(job.Codec, page.VirtualSize);
        var outputBytes = count * BcPackPageSet.BytesPerSymbol;
        buffers.Reserve(length, outputBytes);
        var chunk = buffers.Stored.AsSpan(0, length);
        RandomAccess.Read(file, chunk, page.FileOffset);

        var start = Stopwatch.GetTimestamp();
        var (result, output) = Decode(chunk, count, job, buffers);
        var ticks = Stopwatch.GetTimestamp() - start;
        string? digest = null;

        if (job.Hash && result.Success)
        {
            digest = Convert.ToHexStringLower(SHA256.HashData(output.Span));
        }

        return new BcPackPageOutcome(result, ticks, digest);
    }

    /// <summary>The page's blocks, or its lane symbols for a codec without a block decoder.</summary>
    private static (BcPackDecodeResult Result, ReadOnlyMemory<byte> Output) Decode(ReadOnlySpan<byte> chunk, int count, BcPackScanJob job, BcPackScanBuffers buffers)
    {
        if (count == 0)
        {
            return (new BcPackDecodeResult(BcPackDecodeStatus.InvalidArgument, 0, chunk.Length, 0, 0, 0), ReadOnlyMemory<byte>.Empty);
        }

        if (BcPackPageDecoder.CanDecode((int)job.Codec))
        {
            var size = count * BcPackPageSet.BytesPerSymbol;
            var blocks = BcPackPageDecoder.Decode((int)job.Codec, chunk, size, buffers.Blocks, job.Engine);

            return (blocks, buffers.Blocks.AsMemory(0, size));
        }

        var symbols = (long)BcPackDecoder.Lanes(job.Codec) * count;
        buffers.ReserveSymbols(symbols);
        var result = BcPackDecoder.DecodeSymbols(chunk, job.Codec, count, buffers.Symbols, job.Engine);
        var bytes = MemoryMarshal.AsBytes(buffers.Symbols.AsSpan(0, (int)symbols)).ToArray();

        return (result, bytes);
    }

    /// <summary>A worker's buffers live until the scan ends.</summary>
    private static void KeepBuffers(BcPackScanBuffers buffers)
    {
    }

    private static string Describe(BcPackCodec codec, TexturePage page, BcPackDecodeResult result)
    {
        if (BcPackDecoder.SymbolsPerLane(codec, page.VirtualSize) == 0)
        {
            return $"page virtual size {page.VirtualSize} is not a whole number of blocks";
        }

        return result.Describe();
    }

    private static List<string> HashLines(List<string> files, List<(int File, TexturePage Page)> pages, BcPackPageOutcome[] outcomes)
    {
        var lines = new List<string>(pages.Count);

        for (var index = 0; index < pages.Count; index++)
        {
            var (file, page) = pages[index];
            var result = outcomes[index].Result;
            var status = result.Success ? "ok" : result.Status.ToString();
            var address = page.VirtualAddress.ToString("x", CultureInfo.InvariantCulture);
            lines.Add($"{Path.GetFullPath(files[file])}\t{address}\t{status}\t{outcomes[index].Hash ?? NoHash}\t{result.UnfittedBlocks}");
        }

        return lines;
    }

    private BcPackScanTally PrintFiles(List<string> files, BcPackScanTally[] tallies, bool quiet)
    {
        var total = new BcPackScanTally();

        for (var index = 0; index < files.Count; index++)
        {
            var tally = tallies[index];
            total.Merge(tally);

            if (quiet || tally.Pages == 0)
            {
                continue;
            }

            var megabytes = DecimalText.Fixed(tally.BlockBytes / BytesPerMegabyte, 1);
            _output.WriteLine($"{Path.GetFileName(files[index]),-FileNameWidth} {tally.Converged,CountWidth} converged {tally.Failed,FailedWidth} failed {tally.Unfitted,FailedWidth} unfitted-slot pages {megabytes} MB");
        }

        return total;
    }

    private void PrintFailures(List<string> failures)
    {
        failures.Sort(StringComparer.Ordinal);

        foreach (var line in failures.Take(ListedFailures))
        {
            _output.WriteLine(line);
        }
    }

    private void PrintTotals(BcPackScanJob job, int files, int jobs, BcPackScanTally total, (double Wall, double Decode) seconds)
    {
        var blockMegabytes = total.BlockBytes / BytesPerMegabyte;
        var outGigabytes = DecimalText.Fixed(total.BlockBytes / BytesPerGigabyte, TwoDecimals);
        var storedGigabytes = DecimalText.Fixed(total.StoredBytes / BytesPerGigabyte, TwoDecimals);
        var wallRate = DecimalText.Fixed(blockMegabytes / seconds.Wall, 0);
        var threadRate = DecimalText.Fixed(blockMegabytes / seconds.Decode, 1);
        var name = BcPackPageSet.CodecName(job.Codec);

        _output.WriteLine($"{BcPackPageSet.EngineName(job.Engine)} engine: {files} files, {total.Pages} {name} pages: {total.Converged} converged, {total.Failed} failed; {total.Unfitted} pages touch an unfitted preset slot");
        _output.WriteLine($"  {outGigabytes} GB {name} from {storedGigabytes} GB stored; wall {DecimalText.Fixed(seconds.Wall, 1)} s on {jobs} threads = {wallRate} MB/s with reads; decode time over all threads {DecimalText.Fixed(seconds.Decode, 1)} s = {threadRate} MB/s per thread");
    }
}
