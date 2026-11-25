// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Formats.Ttpf.BcPack;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Storage;

/// <summary>
/// The CPU stand-in for the console's BCPack decoder: a request whose compression names a BCPack codec
/// (<see cref="StorageCompression"/>) carries one stored <c>.ttpf</c> page, and its destination gets the page's BC
/// blocks in stored order, the texture's memory layout. With the user's tables (<see cref="LoadTables"/>) every codec
/// decodes exactly. Without them BC1 decodes with our fitted tables, BC4 and BC5 approximately, and BC3, BC6H and BC7
/// have no block decoder: their pages, and pages that do not decode, get neutral placeholder blocks
/// (<see cref="BcPackPlaceholder"/>), or fail with E_NOTIMPL / ERROR_INVALID_DATA when <see cref="FailMissing"/> is set.
/// Shared by every queue's worker; each logs once per codec.
/// </summary>
internal sealed class BcPackPages
{
    private const int CodecSlots = (int)BcPackCodec.Bc7 + 1;

    private readonly LoaderLog _log;
    private readonly int[] _loggedApproximate = new int[CodecSlots];
    private readonly int[] _loggedMissing = new int[CodecSlots];
    private readonly int[] _loggedFailure = new int[CodecSlots];
    private long _exact;
    private long _approximate;
    private long _placeholders;
    private long _failed;

    public BcPackPages(LoaderLog log)
    {
        _log = log;
    }

    /// <summary>Gets or sets whether pages without exact or approximate blocks fail instead of getting placeholders (<c>--bcpack-missing fail</c>).</summary>
    public bool FailMissing { get; set; }

    /// <summary>Installs the tables of <paramref name="path"/> (<c>--bcpack-tables</c>) for every decode; null or a bad file logs what decodes without them.</summary>
    public void LoadTables(string? path)
    {
        if (path is null)
        {
            _log.Warn("  [xcrdapi] no --bcpack-tables: BCPack BC3, BC6H and BC7 pages get placeholder blocks, BC4 and BC5 decode approximately");

            return;
        }

        try
        {
            BcPackPageDecoder.Install(BcPackTables.Load(path));
            _log.Info($"  [xcrdapi] BCPack tables {path}: every codec decodes exactly");
        }
        catch (Exception exception) when (exception is IOException or UnauthorizedAccessException or InvalidDataException)
        {
            _log.Warn($"  [xcrdapi] --bcpack-tables {path} not used ({exception.Message}): BC3, BC6H and BC7 pages get placeholder blocks");
        }
    }

    /// <summary>One line of totals: pages decoded exactly, approximately, as placeholders, and failed.</summary>
    public string Summary() =>
        $"BCPack pages: {Interlocked.Read(ref _exact)} exact, {Interlocked.Read(ref _approximate)} approximate, {Interlocked.Read(ref _placeholders)} placeholder, {Interlocked.Read(ref _failed)} failed";

    /// <summary>Decodes a page of <paramref name="codec"/> into <paramref name="output"/> (the request's destination size); returns an HRESULT.</summary>
    [HotPath]
    public int Decode(int codec, ReadOnlySpan<byte> page, Span<byte> output)
    {
        if (!BcPackPageDecoder.CanDecode(codec))
        {
            LogMissing(codec);

            return Fallback(codec, output, HResult.NotImplemented);
        }

        var result = BcPackPageDecoder.Decode(codec, page, output.Length, output);

        if (!result.Success)
        {
            LogFailure(codec, result);

            return Fallback(codec, output, HResult.InvalidData);
        }

        if (BcPackPageDecoder.IsApproximate(codec))
        {
            Interlocked.Increment(ref _approximate);
            LogApproximate(codec);
        }
        else
        {
            Interlocked.Increment(ref _exact);
        }

        return HResult.Ok;
    }

    [HotPath]
    private int Fallback(int codec, Span<byte> output, int failure)
    {
        if (FailMissing || !BcPackPlaceholder.TryFill(codec, output))
        {
            Interlocked.Increment(ref _failed);

            return failure;
        }

        Interlocked.Increment(ref _placeholders);

        return HResult.Ok;
    }

    private void LogMissing(int codec)
    {
        if (codec >= CodecSlots || Interlocked.Exchange(ref _loggedMissing[codec], 1) != 0)
        {
            return;
        }

        string outcome = "written as placeholder blocks";

        if (FailMissing)
        {
            outcome = "failed (--bcpack-missing fail)";
        }

        _log.Warn($"  [xcrdapi] BCPack codec {codec} ({(BcPackCodec)codec}) has no block decoder: its pages are {outcome}");
    }

    private void LogFailure(int codec, BcPackDecodeResult result)
    {
        if (Interlocked.Exchange(ref _loggedFailure[codec], 1) != 0)
        {
            return;
        }

        _log.Warn($"  [xcrdapi] BCPack codec {codec} page did not decode ({result.Describe()}): placeholder or failure per --bcpack-missing");
    }

    private void LogApproximate(int codec)
    {
        if (Interlocked.Exchange(ref _loggedApproximate[codec], 1) != 0)
        {
            return;
        }

        _log.Info($"  [xcrdapi] BCPack codec {codec} ({(BcPackCodec)codec}) pages decode with fitted tables: their blocks are approximate");
    }
}
