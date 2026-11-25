// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;
using Xunit;

namespace FiveStar.Loader.Tests.Support;

/// <summary>
/// Drives the xapu.dll shims the way the game's Opus voice does: connect, a create command, then per packet a copy
/// into the slot's staging area, a decode command and a dequeue of its result.
/// </summary>
internal sealed unsafe class XApuDriver
{
    private const int Slot = 5;
    private const int Record = 0x40;
    private const int Result = 0x50;
    private const ulong Context = 0x1234_5678_9ABC;
    private const int InputSlot = 0x280;
    private const int OutputBase = 0x14000;
    private const int OutputSlot = 0x1E00;
    private const int ChannelBytes = 0xF00;
    private const int Frame = 960;

    private readonly delegate* unmanaged<nint, uint, uint> _count;
    private readonly delegate* unmanaged<nint, byte*, uint, int> _enqueueRaw;
    private readonly delegate* unmanaged<nint, byte*, int> _dequeueRaw;
    private readonly nint _memory;
    private readonly nint _handle;
    private readonly uint _stream;
    private readonly int _channels;

    public XApuDriver(TestRuntime test, uint stream, int channels)
    {
        var connect = (delegate* unmanaged<nint, nint*, nint*, int>)test.Shim("xapu.dll", "XApuConnect");
        _enqueueRaw = (delegate* unmanaged<nint, byte*, uint, int>)test.Shim("xapu.dll", "XApuEnqueueCommand");
        _dequeueRaw = (delegate* unmanaged<nint, byte*, int>)test.Shim("xapu.dll", "XApuDequeueResult");
        _count = (delegate* unmanaged<nint, uint, uint>)test.Shim("xapu.dll", "XApuGetResultCount");
        _stream = stream;
        _channels = channels;

        nint memory;
        nint handle;
        Assert.Equal(0, connect(0, &memory, &handle));
        _memory = memory;
        _handle = handle;

        Send(1, 0, 0, 0, 0, 0);
        Assert.Equal(1u, Receive(out _));
    }

    /// <summary>Decodes one packet, dropping its first <paramref name="first"/> samples; returns the interleaved floats the result reports.</summary>
    public float[] Decode(ReadOnlySpan<byte> packet, uint first = 0)
    {
        nint input = _memory + (Slot * InputSlot);
        nint output = _memory + OutputBase + (Slot * OutputSlot);
        packet.CopyTo(new Span<byte>((void*)input, packet.Length));

        Send(3, first, packet.Length, input, _channels * ChannelBytes, output);
        Assert.Equal(1u, _count(_handle, 0));
        Assert.Equal(3u, Receive(out int bytes));

        return new ReadOnlySpan<float>((void*)output, bytes / sizeof(float)).ToArray();
    }

    private void Send(uint kind, uint first, int inputBytes, nint input, int outputBytes, nint output)
    {
        var record = stackalloc byte[Record];
        var span = new Span<byte>(record, Record);
        span.Clear();

        BinaryPrimitives.WriteUInt32LittleEndian(span, kind);
        BinaryPrimitives.WriteUInt32LittleEndian(span[0x04..], _stream);
        BinaryPrimitives.WriteUInt64LittleEndian(span[0x10..], Context);
        BinaryPrimitives.WriteUInt32LittleEndian(span[0x20..], first);
        BinaryPrimitives.WriteUInt32LittleEndian(span[0x24..], Frame);

        BinaryPrimitives.WriteInt32LittleEndian(span[0x28..], inputBytes);
        BinaryPrimitives.WriteInt64LittleEndian(span[0x2C..], input);
        BinaryPrimitives.WriteInt32LittleEndian(span[0x34..], outputBytes);
        BinaryPrimitives.WriteInt64LittleEndian(span[0x38..], output);

        Assert.Equal(0, _enqueueRaw(_handle, record, 0));
    }

    private uint Receive(out int bytes)
    {
        var record = stackalloc byte[Result];
        Assert.Equal(0, _dequeueRaw(_handle, record));
        var span = new ReadOnlySpan<byte>(record, Result);

        Assert.Equal(_stream, BinaryPrimitives.ReadUInt32LittleEndian(span[0x04..]));
        Assert.Equal(Context, BinaryPrimitives.ReadUInt64LittleEndian(span[0x10..]));
        bytes = BinaryPrimitives.ReadInt32LittleEndian(span[0x34..]);
        Assert.Equal(bytes, BinaryPrimitives.ReadInt32LittleEndian(span[0x30..]) * _channels * sizeof(float));

        return BinaryPrimitives.ReadUInt32LittleEndian(span);
    }
}
