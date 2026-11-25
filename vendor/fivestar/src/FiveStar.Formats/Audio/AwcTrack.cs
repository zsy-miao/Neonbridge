// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Audio;

/// <summary>
/// One playable sound: a single stream, or one channel of a multichannel stream. Opus tracks carry their packets;
/// other codecs carry the raw data range.
/// </summary>
/// <param name="ContainerOffset">Archive offset of the container.</param>
/// <param name="Id">The stream (or channel) id.</param>
/// <param name="Format">Samples, sample rate, headroom and codec.</param>
/// <param name="DataOffset">Archive offset of the data chunk.</param>
/// <param name="DataSize">Bytes of the data chunk that belong to this track (a channel's share of a multichannel chunk).</param>
/// <param name="Packets">The Opus packets in play order; empty for other codecs.</param>
/// <param name="Problem">Why the track does not read cleanly, or null.</param>
public sealed record AwcTrack(
    long ContainerOffset,
    uint Id,
    AwcFormat Format,
    long DataOffset,
    long DataSize,
    IReadOnlyList<AwcPacket> Packets,
    string? Problem)
{
    /// <summary>Gets the codec, as the enum when it is one of the known values.</summary>
    public AwcCodec Codec => (AwcCodec)Format.Codec;

    /// <summary>Total samples at 48 kHz of the Opus packets.</summary>
    public long PacketSamples
    {
        get
        {
            long total = 0;

            foreach (var packet in Packets)
            {
                total += packet.Samples;
            }

            return total;
        }
    }
}
