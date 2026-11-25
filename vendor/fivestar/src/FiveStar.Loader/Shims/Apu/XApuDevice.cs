// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;
using System.Runtime.InteropServices;
using Concentus;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Apu;

/// <summary>
/// xapu.dll on the CPU: the command queue the game drives its Opus voices through. On the console the audio
/// processor decodes the packets; here a command runs when it is enqueued and its result waits in a queue for
/// XApuGetResultCount and XApuDequeueResult, which the game polls every audio frame. Opus runs on Concentus.
/// </summary>
internal sealed unsafe class XApuDevice : IDisposable
{
    /// <summary>The connection handle; any nonzero value, the game only passes it back.</summary>
    public const nint ConnectionHandle = 0x58415055;

    /// <summary>Distinct pitch and first-sample pairs logged.</summary>
    private const int LoggedFieldValues = 8;

    private readonly object _gate = new();
    private readonly Queue<byte[]> _results = new();
    private readonly Dictionary<uint, OpusVoice> _voices = [];
    private readonly HashSet<(float Pitch, uint First)> _loggedFields = [];
    private readonly AudioCounters _counters;
    private readonly LoaderLog _log;
    private nint _memory;
    private bool _decodeFailureLogged;

    public XApuDevice(AudioCounters counters, LoaderLog log)
    {
        _counters = counters;
        _log = log;
    }

    /// <summary>The shared memory of the connection, 0 before XApuConnect.</summary>
    public nint SharedMemory => _memory;

    /// <summary>HRESULT XApuConnect(const void* config, void** sharedMemory, XAPU_HANDLE* handle); the config is not read.</summary>
    public int Connect(nint* sharedMemory, nint* handle)
    {
        if (sharedMemory == null || handle == null)
        {
            return HResult.Pointer;
        }

        lock (_gate)
        {
            if (_memory == 0)
            {
                _memory = (nint)NativeMemory.AllocZeroed(XApuLayout.SharedMemoryBytes);
            }
        }

        *sharedMemory = _memory;
        *handle = ConnectionHandle;
        _log.Info($"  [xapu] XApuConnect -> shared memory 0x{_memory:x} (0x{XApuLayout.SharedMemoryBytes:x} bytes), Opus on the CPU");

        return HResult.Ok;
    }

    /// <summary>HRESULT XApuEnqueueCommand(XAPU_HANDLE, const command*, UINT32); runs the command and queues its result.</summary>
    public int Enqueue(nint handle, byte* record)
    {
        if (handle != ConnectionHandle || record == null)
        {
            return HResult.InvalidArgument;
        }

        var source = new ReadOnlySpan<byte>(record, XApuLayout.RecordSize);
        var command = XApuCommand.Read(source);
        var result = new byte[XApuLayout.ResultSize];
        source[..XApuLayout.IdentityBytes].CopyTo(result);

        lock (_gate)
        {
            Run(command, result);
            _results.Enqueue(result);
        }

        return HResult.Ok;
    }

    /// <summary>UINT32 XApuGetResultCount(XAPU_HANDLE, UINT32): results waiting to be dequeued.</summary>
    public uint ResultCount(nint handle)
    {
        if (handle != ConnectionHandle)
        {
            return 0;
        }

        lock (_gate)
        {
            return (uint)_results.Count;
        }
    }

    /// <summary>HRESULT XApuDequeueResult(XAPU_HANDLE, result*): the oldest result; E_FAIL when there is none.</summary>
    public int Dequeue(nint handle, byte* record)
    {
        if (handle != ConnectionHandle || record == null)
        {
            return HResult.InvalidArgument;
        }

        byte[]? result;

        lock (_gate)
        {
            if (!_results.TryDequeue(out result))
            {
                return HResult.Fail;
            }
        }

        result.CopyTo(new Span<byte>(record, XApuLayout.ResultSize));
        _counters.OpusResultTaken();

        return HResult.Ok;
    }

    /// <summary>XApuDisconnect: drops every stream and pending result; the shared memory stays for late readers.</summary>
    public void Disconnect()
    {
        lock (_gate)
        {
            _voices.Clear();
            _results.Clear();
        }
    }

    public void Dispose()
    {
        NativeMemory.Free((void*)_memory);
        _memory = 0;
    }

    private void Run(in XApuCommand command, byte[] result)
    {
        switch (command.Kind)
        {
            case XApuCommandKind.Create:
                _voices[command.Stream] = new OpusVoice();
                _counters.OpusCreated();
                break;
            case XApuCommandKind.Destroy:
                _voices.Remove(command.Stream);
                _counters.OpusDestroyed();
                break;
            case XApuCommandKind.Reset:
                Voice(command.Stream).Reset();
                break;
            case XApuCommandKind.Decode:
                WriteDecodeResult(command, Decode(command), result);
                break;
            default:
                _log.Warn($"[xapu] unknown command {(uint)command.Kind} for stream {command.Stream}");
                break;
        }
    }

    /// <summary>The result of a decode: samples per channel and bytes written, the output, pitch and packet echoed.</summary>
    private static void WriteDecodeResult(in XApuCommand command, (int Samples, int Channels) decoded, byte[] result)
    {
        var span = result.AsSpan();
        BinaryPrimitives.WriteInt32LittleEndian(span[XApuLayout.ResultSamplesOffset..], decoded.Samples);
        BinaryPrimitives.WriteInt32LittleEndian(span[XApuLayout.ResultBytesOffset..], decoded.Samples * decoded.Channels * sizeof(float));
        BinaryPrimitives.WriteInt64LittleEndian(span[XApuLayout.ResultOutputAddressOffset..], command.OutputAddress);

        BinaryPrimitives.WriteSingleLittleEndian(span[XApuLayout.ResultPitchOffset..], 1f);
        BinaryPrimitives.WriteUInt32LittleEndian(span[XApuLayout.ResultInputBytesOffset..], command.InputBytes);
        BinaryPrimitives.WriteInt64LittleEndian(span[XApuLayout.ResultInputAddressOffset..], command.InputAddress);
    }

    /// <summary>Decodes one packet; the samples per channel written (0 for an empty or broken packet) and the channels.</summary>
    private (int Samples, int Channels) Decode(in XApuCommand command)
    {
        uint frame = command.FrameSamples == 0 ? XApuLayout.DefaultFrameSamples : command.FrameSamples;
        int channels = (int)Math.Clamp(command.OutputBytes / (XApuLayout.DefaultFrameSamples * sizeof(float)), 1, XApuLayout.MaxChannels);
        int inputBytes = (int)Math.Min(command.InputBytes, (uint)XApuLayout.InputSlotBytes);
        int outputFloats = (int)Math.Min(command.OutputBytes, (uint)XApuLayout.OutputSlotBytes) / sizeof(float);
        LogFields(command);

        if (command.InputAddress == 0 || command.OutputAddress == 0 || inputBytes == 0)
        {
            _counters.OpusDecoded(0, true);

            return (0, channels);
        }

        var packet = new ReadOnlySpan<byte>((void*)command.InputAddress, inputBytes);
        var output = new Span<float>((void*)command.OutputAddress, outputFloats);

        try
        {
            int samples = Voice(command.Stream).Decode(packet, output, channels, (int)Math.Min(command.FirstSample, XApuLayout.MaxPacketSamples), (int)frame);
            _counters.OpusDecoded(samples, false);

            return (samples, channels);
        }
        catch (OpusException e)
        {
            if (!_decodeFailureLogged)
            {
                _decodeFailureLogged = true;
                _log.Warn($"[xapu] Opus packet of stream {command.Stream} did not decode ({e.Message}); logged once");
            }

            _counters.OpusDecoded(0, true);

            return (0, channels);
        }
    }

    /// <summary>Logs the first few distinct pitch and first-sample values the game sends.</summary>
    private void LogFields(in XApuCommand command)
    {
        if (_loggedFields.Count < LoggedFieldValues && _loggedFields.Add((command.Pitch, command.FirstSample)))
        {
            _log.Info($"  [xapu] decode of stream {command.Stream}: pitch field {command.Pitch}, first sample {command.FirstSample}, keep {command.FrameSamples}");
        }
    }

    /// <summary>The stream's voice; a stream the game did not create gets one, as the hardware would decode it anyway.</summary>
    private OpusVoice Voice(uint stream)
    {
        if (!_voices.TryGetValue(stream, out var voice))
        {
            voice = new OpusVoice();
            _voices[stream] = voice;
        }

        return voice;
    }
}
