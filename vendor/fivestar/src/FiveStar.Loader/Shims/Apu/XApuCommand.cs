// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;

namespace FiveStar.Loader.Shims.Apu;

/// <summary>One xapu.dll command record (<see cref="XApuLayout"/>).</summary>
internal readonly record struct XApuCommand(
    XApuCommandKind Kind,
    uint Stream,
    uint Channels,
    float Pitch,
    uint FirstSample,
    uint FrameSamples,
    uint InputBytes,
    nint InputAddress,
    uint OutputBytes,
    nint OutputAddress)
{
    public static XApuCommand Read(ReadOnlySpan<byte> record)
    {
        return new XApuCommand(
            (XApuCommandKind)BinaryPrimitives.ReadUInt32LittleEndian(record[XApuLayout.KindOffset..]),
            BinaryPrimitives.ReadUInt32LittleEndian(record[XApuLayout.StreamOffset..]),
            BinaryPrimitives.ReadUInt32LittleEndian(record[XApuLayout.CreateChannelsOffset..]),
            BinaryPrimitives.ReadSingleLittleEndian(record[XApuLayout.PitchOffset..]),
            BinaryPrimitives.ReadUInt32LittleEndian(record[XApuLayout.FirstSampleOffset..]),
            BinaryPrimitives.ReadUInt32LittleEndian(record[XApuLayout.FrameSamplesOffset..]),
            BinaryPrimitives.ReadUInt32LittleEndian(record[XApuLayout.InputBytesOffset..]),
            (nint)BinaryPrimitives.ReadInt64LittleEndian(record[XApuLayout.InputAddressOffset..]),
            BinaryPrimitives.ReadUInt32LittleEndian(record[XApuLayout.OutputBytesOffset..]),
            (nint)BinaryPrimitives.ReadInt64LittleEndian(record[XApuLayout.OutputAddressOffset..]));
    }
}
