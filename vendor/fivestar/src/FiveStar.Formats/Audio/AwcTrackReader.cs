// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Formats.Common;

namespace FiveStar.Formats.Audio;

/// <summary>
/// Turns the streams of a container into tracks: a stream with a <see cref="AwcChunkTag.Format"/> and a
/// <see cref="AwcChunkTag.Data"/> chunk is one track; a stream with a <see cref="AwcChunkTag.StreamFormat"/> chunk is
/// one track per channel. Streams without either (the channel names of a multichannel stream) give none.
/// </summary>
public sealed class AwcTrackReader
{
    private const int StereoChannels = 2;

    private readonly IByteSource _source;

    public AwcTrackReader(IByteSource source)
    {
        _source = source;
    }

    public IEnumerable<AwcTrack> Read(AwcContainer container)
    {
        foreach (var stream in container.Streams)
        {
            var data = stream.Find(AwcChunkTag.Data);
            var format = stream.Find(AwcChunkTag.Format);
            var streamFormat = stream.Find(AwcChunkTag.StreamFormat);

            if (data is null)
            {
                continue;
            }

            if (format is not null)
            {
                yield return ReadSingle(container.Offset, stream.Id, format.Value, data.Value);
            }
            else if (streamFormat is not null)
            {
                foreach (var track in ReadMultichannel(container.Offset, streamFormat.Value, data.Value))
                {
                    yield return track;
                }
            }
        }
    }

    private AwcTrack ReadSingle(long containerOffset, uint id, AwcChunk formatChunk, AwcChunk data)
    {
        var dataOffset = containerOffset + data.Offset;
        var format = AwcFormat.Read(_source.Read(containerOffset + formatChunk.Offset, formatChunk.Size));

        if (format is null)
        {
            return new AwcTrack(containerOffset, id, new AwcFormat(0, 1, 0, 0, 1), dataOffset, data.Size, [], "format chunk");
        }

        if (format.Codec != (byte)AwcCodec.Opus)
        {
            return new AwcTrack(containerOffset, id, format, dataOffset, data.Size, [], null);
        }

        var packets = new List<AwcPacket>();
        string? problem = null;

        if (!OpusPageReader.TryRead(_source.Read(dataOffset, data.Size), dataOffset, packets))
        {
            problem = "bad page";
        }

        return new AwcTrack(containerOffset, id, format, dataOffset, data.Size, packets, problem ?? CheckChannels(format, packets));
    }

    /// <summary>The stereo bit of the first packet against the format's channel count.</summary>
    private string? CheckChannels(AwcFormat format, IReadOnlyList<AwcPacket> packets)
    {
        if (packets.Count == 0)
        {
            return "no packets";
        }

        var stereo = OpusPacket.IsStereo(_source.Read(packets[0].Offset, 1));

        if (stereo != (format.Channels == StereoChannels) || format.Channels == 0 || format.Channels > StereoChannels)
        {
            return "channel count";
        }

        return null;
    }

    private List<AwcTrack> ReadMultichannel(long containerOffset, AwcChunk formatChunk, AwcChunk data)
    {
        var dataOffset = containerOffset + data.Offset;
        var format = AwcStreamFormat.Read(_source.Read(containerOffset + formatChunk.Offset, formatChunk.Size));

        if (format is null)
        {
            return [new AwcTrack(containerOffset, 0, new AwcFormat(0, 1, 0, 0, 1), dataOffset, data.Size, [], "stream format chunk")];
        }

        var count = format.Channels.Count;
        var packets = new List<AwcPacket>[count];

        for (var index = 0; index < count; index++)
        {
            packets[index] = [];
        }

        string? problem = "multichannel codec";

        if (format.Channels.All(channel => channel.Format.Codec == (byte)AwcCodec.Opus))
        {
            problem = AwcBlockReader.Read(_source, data, containerOffset, format, packets);
        }

        var tracks = new List<AwcTrack>(count);

        for (var index = 0; index < count; index++)
        {
            var channel = format.Channels[index];
            var trackProblem = problem ?? CheckChannels(channel.Format, packets[index]);
            tracks.Add(new AwcTrack(containerOffset, channel.Id, channel.Format, dataOffset, data.Size / count, packets[index], trackProblem));
        }

        return tracks;
    }
}
