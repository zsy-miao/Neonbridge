// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Cli.Archive;
using FiveStar.Formats.Audio;
using FiveStar.Formats.Common;

namespace FiveStar.Cli.Audio;

/// <summary>Tracks, data bytes and clean tracks per codec, and the problems seen.</summary>
internal sealed class CodecCoverage
{
    private const int NameWidth = 10;
    private const int CountWidth = 10;
    private const int BytesWidth = 16;
    private const int CodecDigits = 2;

    private readonly OrderedMap<string, long[]> _rows = new();

    public Tally<string> Problems { get; } = new();

    public static string CodecName(AwcTrack track)
    {
        var codec = track.Codec;

        if (Enum.IsDefined(codec))
        {
            return codec.ToString().ToLowerInvariant();
        }

        return "codec" + ReportText.Hex(track.Format.Codec, CodecDigits);
    }

    /// <summary>Whether the track reads without a problem and, for Opus, its packets cover the encoder delay and the header's samples.</summary>
    public static bool IsClean(AwcTrack track)
    {
        if (track.Problem is not null)
        {
            return false;
        }

        if (track.Codec == AwcCodec.Opus)
        {
            return track.PacketSamples >= track.Format.Frames + OggOpusWriter.PreSkip;
        }

        return Enum.IsDefined(track.Codec);
    }

    public void Add(AwcTrack track)
    {
        var name = CodecName(track);

        if (!_rows.TryGetValue(name, out var row))
        {
            row = new long[(int)Column.Count];
            _rows.Set(name, row);
        }

        row[(int)Column.Tracks]++;
        row[(int)Column.Bytes] += track.DataSize;

        if (IsClean(track))
        {
            row[(int)Column.Clean]++;
        }
        else
        {
            Problems.Add(track.Problem ?? "short packets");
        }
    }

    public void Print(TextWriter output)
    {
        output.WriteLine($"{"codec",-NameWidth} {"tracks",CountWidth} {"data bytes",BytesWidth} {"clean",CountWidth}");

        foreach (var name in _rows.Keys)
        {
            var row = _rows[name];
            output.WriteLine($"{name,-NameWidth} {row[(int)Column.Tracks],CountWidth} {row[(int)Column.Bytes],BytesWidth} {row[(int)Column.Clean],CountWidth}");
        }

        output.WriteLine("problems " + ReportText.CountMap(Problems));
    }

    private enum Column
    {
        Tracks,
        Bytes,
        Clean,
        Count,
    }
}
