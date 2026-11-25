// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;
using System.Runtime.InteropServices;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Apu;

/// <summary>
/// chad.dll (XDsp) on the CPU: the audio DSP the game runs its convolution effects on. A stream is activated with an
/// effect config and gets a status record; each submitted command processes one block from an input page into an
/// output page and completes by advancing the status record's completed count, which the game polls. Here a command
/// runs and completes when it is submitted (<see cref="XDspStream"/>). The convolution runs only when
/// <see cref="Convolve"/> is set, because the impulse response format is inferred; otherwise every effect writes
/// silence, as the game's own fallback does while a result is late.
/// </summary>
internal sealed unsafe class XDspDevice : IDisposable
{
    /// <summary>The connection handle; any nonzero value, the game only passes it back.</summary>
    public const nint ConnectionHandle = 0x58445350;

    /// <summary>Stream handles are this plus the stream's index, so a handle the game passes back is checked cheaply.</summary>
    public const nint StreamHandleBase = 0x58440000;

    private readonly object _gate = new();
    private readonly XDspStream?[] _streams = new XDspStream?[XDspLayout.MaxStreams];
    private readonly uint[] _completed = new uint[XDspLayout.MaxStreams];
    private readonly LoaderLog _log;
    private byte* _status;
    private uint _streamLimit;
    private long _submitted;
    private int _firstSubmitLogged;

    public XDspDevice(LoaderLog log)
    {
        _log = log;
    }

    /// <summary>Commands completed since start.</summary>
    public long Submitted => Interlocked.Read(ref _submitted);

    /// <summary>Whether convolution streams convolve (<c>--xdsp-convolution</c>); read when a stream is activated.</summary>
    public bool Convolve { get; set; }

    /// <summary>
    /// HRESULT XDspConnectWithMaximumStreamLimit(void* memory, UINT32 bytes, UINT32 maxStreams, UINT32 (0x80),
    /// UINT32 (1), XDSP_HANDLE* handle). The memory is the game's own page pool, which the DSP reads and writes.
    /// </summary>
    public int Connect(nint memory, uint bytes, uint maxStreams, nint* handle)
    {
        if (handle == null)
        {
            return HResult.Pointer;
        }

        lock (_gate)
        {
            if (_status == null)
            {
                _status = (byte*)NativeMemory.AllocZeroed(XDspLayout.MaxStreams * XDspLayout.StatusSize);
            }

            _streamLimit = Math.Min(maxStreams, XDspLayout.MaxStreams);
        }

        *handle = ConnectionHandle;
        string effects = Convolve ? "convolution on the CPU" : "effects pass silence";
        _log.Info($"  [xdsp] XDspConnect: memory 0x{memory:x} (0x{bytes:x} bytes), {maxStreams} streams; {effects}");

        return HResult.Ok;
    }

    /// <summary>HRESULT XDspActivate(XDSP_HANDLE, const config*, status** status, XDSP_STREAM* stream).</summary>
    public int Activate(nint handle, byte* config, nint* status, nint* stream)
    {
        if (handle != ConnectionHandle || config == null || status == null || stream == null)
        {
            return HResult.InvalidArgument;
        }

        var effect = XDspStream.Create(new ReadOnlySpan<byte>(config, XDspLayout.ConfigSize), Convolve);
        int index = Claim(effect);

        if (index < 0)
        {
            return HResult.OutOfMemory;
        }

        *status = (nint)Status(index);
        *stream = StreamHandleBase + index;
        LogActivation(index, effect, new ReadOnlySpan<byte>(config, XDspLayout.ConfigSize));

        return HResult.Ok;
    }

    /// <summary>
    /// HRESULT XDspSubmitCommand(XDSP_STREAM, const command*, UINT32* id): runs the command, completes it at once and
    /// returns its id, the value the stream's completed count now holds.
    /// </summary>
    [HotPath]
    public int Submit(nint stream, byte* command, uint* id)
    {
        int index = Index(stream);

        if (index < 0 || command == null)
        {
            return HResult.InvalidArgument;
        }

        var effect = Volatile.Read(ref _streams[index]);

        if (effect is null)
        {
            return HResult.InvalidArgument;
        }

        effect.Process(command);
        uint completed = Interlocked.Increment(ref _completed[index]);
        byte* status = Status(index);
        *(uint*)(status + XDspLayout.StatusErrorOffset) = 0;
        Volatile.Write(ref *(uint*)(status + XDspLayout.StatusCompletedOffset), completed);

        if (id != null)
        {
            *id = completed;
        }

        Interlocked.Increment(ref _submitted);
        LogFirstSubmit(index, command);

        return HResult.Ok;
    }

    /// <summary>XDspDeactivate(XDSP_STREAM): frees the stream; the result is not read.</summary>
    public void Deactivate(nint stream)
    {
        int index = Index(stream);

        if (index < 0)
        {
            return;
        }

        lock (_gate)
        {
            Volatile.Write(ref _streams[index], null);
        }
    }

    /// <summary>XDspDisconnect(XDSP_HANDLE): drops every stream; the status records stay for late readers.</summary>
    public void Disconnect()
    {
        lock (_gate)
        {
            Array.Clear(_streams);
        }

        _log.Info($"  [xdsp] XDspDisconnect after {Submitted} commands");
    }

    public void Dispose()
    {
        NativeMemory.Free(_status);
        _status = null;
    }

    private int Claim(XDspStream effect)
    {
        lock (_gate)
        {
            for (int index = 0; index < _streamLimit; index++)
            {
                if (_streams[index] is null)
                {
                    _completed[index] = 0;
                    new Span<byte>(Status(index), XDspLayout.StatusSize).Clear();
                    Volatile.Write(ref _streams[index], effect);

                    return index;
                }
            }
        }

        return -1;
    }

    private int Index(nint stream)
    {
        nint index = stream - StreamHandleBase;

        if (index < 0 || index >= XDspLayout.MaxStreams || _status == null)
        {
            return -1;
        }

        return (int)index;
    }

    private byte* Status(int index)
    {
        return _status + (index * XDspLayout.StatusSize);
    }

    private void LogActivation(int index, XDspStream effect, ReadOnlySpan<byte> config)
    {
        uint frames = BinaryPrimitives.ReadUInt32LittleEndian(config[XDspLayout.ConfigFramesOffset..]);
        uint channels = BinaryPrimitives.ReadUInt32LittleEndian(config[XDspLayout.ConfigChannelsOffset..]);
        uint response = BinaryPrimitives.ReadUInt32LittleEndian(config[XDspLayout.ConfigResponseLengthOffset..]);
        string work = effect.Channels > 0 ? $"convolving {effect.Channels} x {effect.BlockFrames} frames, {effect.Partitions} partitions" : "silent";
        _log.Info($"  [xdsp] XDspActivate stream {index}: {effect.Kind}, {frames} frames, {channels} channels, response {response} floats; {work}");
    }

    [HotPath]
    private void LogFirstSubmit(int index, byte* command)
    {
        if (_firstSubmitLogged != 0 || Interlocked.Exchange(ref _firstSubmitLogged, 1) != 0)
        {
            return;
        }

        LogCommand(index, command);
    }

    private void LogCommand(int index, byte* command)
    {
        var record = new ReadOnlySpan<byte>(command, XDspLayout.CommandSize);
        long input = BinaryPrimitives.ReadInt64LittleEndian(record[XDspLayout.CommandInputOffset..]);
        long output = BinaryPrimitives.ReadInt64LittleEndian(record[XDspLayout.CommandOutputOffset..]);
        _log.Info($"  [xdsp] first command on stream {index}: input 0x{input:x}, output 0x{output:x}, completed at once");
    }
}
