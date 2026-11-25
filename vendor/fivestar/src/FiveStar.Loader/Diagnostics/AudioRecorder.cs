// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;
using System.Diagnostics;
using System.Runtime.InteropServices;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Diagnostics;

/// <summary>
/// <c>--audio-capture &lt;file.wav&gt;</c>: records what the game plays, 48 kHz stereo 16-bit, through WASAPI process
/// loopback (the loader's process tree only). It runs in a child copy of the loader (<c>--record-audio</c>), so the
/// recorder's own audio client does not change the game's process. Stretches where the game played nothing are written
/// as silence, so the file keeps real time; the header is rewritten every second, so a killed run leaves a valid file.
/// At the end it prints an <see cref="AudioActivity"/> summary.
/// </summary>
internal static unsafe class AudioRecorder
{
    private const int FrameBytes = AudioActivity.Channels * sizeof(short);
    private const int HeaderBytes = 44;
    private const int FormatBytes = 16;
    private const ushort PcmTag = 1;
    private const ushort BitsPerSample = 16;
    private const long BufferDuration = 10_000_000;
    private const long TicksPerSecond = 10_000_000;
    private const int PollMilliseconds = 10;
    private const int HeaderMilliseconds = 1000;

    /// <summary>Gaps shorter than this many frames (2 ms) are jitter, not silence.</summary>
    private const long GapFrames = AudioActivity.Rate / 500;

    public static int Run(int processId, string path, TextWriter output)
    {
        var activity = new AudioActivity();

        try
        {
            Record(processId, path, activity, output);
        }
        catch (Exception e) when (e is InvalidOperationException or IOException or ArgumentException)
        {
            output.WriteLine($"[audio-capture] stopped: {e.Message}");
        }

        output.WriteLine($"[audio-capture] {path}: {activity.Summary()}");

        return 0;
    }

    private static void Record(int processId, string path, AudioActivity activity, TextWriter output)
    {
        using var target = Process.GetProcessById(processId);
        LoopbackActivation.Check(Ole32.CoInitializeEx(0, Ole32.MultiThreaded), "CoInitializeEx");
        nint capture = Start(LoopbackActivation.Activate(processId));

        using var file = new FileStream(path, FileMode.Create, FileAccess.Write, FileShare.Read);
        file.Write(Header(0));
        output.WriteLine($"[audio-capture] recording process {processId} to {path}");

        var header = Stopwatch.StartNew();
        long written = 0;
        ulong first = 0;

        while (true)
        {
            Thread.Sleep(PollMilliseconds);
            bool ended = target.HasExited;
            Drain(capture, file, activity, ref written, ref first);

            if (ended || header.ElapsedMilliseconds >= HeaderMilliseconds)
            {
                Patch(file, written);
                header.Restart();
            }

            if (ended)
            {
                return;
            }
        }
    }

    /// <summary>Writes every packet the client holds, after silence for the time nothing was played before it.</summary>
    private static void Drain(nint capture, FileStream file, AudioActivity activity, ref long written, ref ulong first)
    {
        var getBuffer = (delegate* unmanaged<nint, byte**, uint*, uint*, ulong*, ulong*, int>)ComObject.Slot(capture, Mmdevapi.GetBufferSlot);
        var releaseBuffer = (delegate* unmanaged<nint, uint, int>)ComObject.Slot(capture, Mmdevapi.ReleaseBufferSlot);
        var nextPacket = (delegate* unmanaged<nint, uint*, int>)ComObject.Slot(capture, Mmdevapi.GetNextPacketSizeSlot);
        uint packet = 0;

        while (nextPacket(capture, &packet) >= 0 && packet > 0)
        {
            byte* data;
            uint frames;
            uint flags;
            ulong position;
            ulong time;
            LoopbackActivation.Check(getBuffer(capture, &data, &frames, &flags, &position, &time), "IAudioCaptureClient::GetBuffer");

            if (first == 0)
            {
                first = time;
            }

            long at = (long)(time - first) * AudioActivity.Rate / TicksPerSecond;

            if (at > written + GapFrames)
            {
                WriteSilence(file, activity, at - written);
                written = at;
            }

            WriteFrames(file, activity, data, (int)frames, (flags & Mmdevapi.SilentBuffer) != 0);
            written += frames;
            LoopbackActivation.Check(releaseBuffer(capture, frames), "IAudioCaptureClient::ReleaseBuffer");
        }
    }

    /// <summary>Initializes the loopback client for 48 kHz stereo 16-bit and starts it; returns its IAudioCaptureClient.</summary>
    private static nint Start(nint client)
    {
        var format = stackalloc byte[HeaderBytes];
        WriteFormat(new Span<byte>(format, FormatBytes + sizeof(ushort)));

        var initialize = (delegate* unmanaged<nint, int, uint, long, long, byte*, Guid*, int>)ComObject.Slot(client, Mmdevapi.InitializeSlot);
        LoopbackActivation.Check(initialize(client, Mmdevapi.SharedMode, Mmdevapi.LoopbackConvertFlags, BufferDuration, 0, format, null), "IAudioClient::Initialize");

        Guid iid = Mmdevapi.AudioCaptureClient;
        nint capture = 0;
        var getService = (delegate* unmanaged<nint, Guid*, nint*, int>)ComObject.Slot(client, Mmdevapi.GetServiceSlot);
        LoopbackActivation.Check(getService(client, &iid, &capture), "IAudioClient::GetService");

        var start = (delegate* unmanaged<nint, int>)ComObject.Slot(client, Mmdevapi.StartSlot);
        LoopbackActivation.Check(start(client), "IAudioClient::Start");

        return capture;
    }

    private static void WriteFrames(FileStream file, AudioActivity activity, byte* data, int frames, bool silent)
    {
        if (silent || data == null)
        {
            WriteSilence(file, activity, frames);

            return;
        }

        var bytes = new ReadOnlySpan<byte>(data, frames * FrameBytes);
        file.Write(bytes);
        activity.Add(MemoryMarshal.Cast<byte, short>(bytes));
    }

    private static void WriteSilence(FileStream file, AudioActivity activity, long frames)
    {
        var zeros = new byte[frames * FrameBytes];
        file.Write(zeros);
        activity.AddSilence(frames);
    }

    private static void Patch(FileStream file, long frames)
    {
        long end = file.Position;
        file.Position = 0;
        file.Write(Header(frames));
        file.Position = end;
        file.Flush();
    }

    /// <summary>The RIFF WAVE header of <paramref name="frames"/> 48 kHz stereo 16-bit frames.</summary>
    private static byte[] Header(long frames)
    {
        uint data = (uint)Math.Min(frames * FrameBytes, uint.MaxValue - HeaderBytes);
        var header = new byte[HeaderBytes];
        var span = header.AsSpan();

        "RIFF"u8.CopyTo(span);
        BinaryPrimitives.WriteUInt32LittleEndian(span[(int)HeaderField.RiffSize..], data + HeaderBytes - (int)HeaderField.Wave);
        "WAVEfmt "u8.CopyTo(span[(int)HeaderField.Wave..]);
        BinaryPrimitives.WriteUInt32LittleEndian(span[(int)HeaderField.FormatSize..], FormatBytes);
        WriteFormat(span[(int)HeaderField.Format..]);

        "data"u8.CopyTo(span[(int)HeaderField.Data..]);
        BinaryPrimitives.WriteUInt32LittleEndian(span[(int)HeaderField.DataSize..], data);

        return header;
    }

    /// <summary>WAVEFORMATEX for 16-bit PCM (cbSize 0).</summary>
    private static void WriteFormat(Span<byte> format)
    {
        format.Clear();
        BinaryPrimitives.WriteUInt16LittleEndian(format, PcmTag);
        BinaryPrimitives.WriteUInt16LittleEndian(format[(int)FormatField.Channels..], AudioActivity.Channels);

        BinaryPrimitives.WriteUInt32LittleEndian(format[(int)FormatField.Rate..], AudioActivity.Rate);
        BinaryPrimitives.WriteUInt32LittleEndian(format[(int)FormatField.BytesPerSecond..], AudioActivity.Rate * FrameBytes);
        BinaryPrimitives.WriteUInt16LittleEndian(format[(int)FormatField.BlockAlign..], FrameBytes);
        BinaryPrimitives.WriteUInt16LittleEndian(format[(int)FormatField.Bits..], BitsPerSample);
    }

    private enum HeaderField
    {
        RiffSize = 4,
        Wave = 8,
        FormatSize = 16,
        Format = 20,
        Data = 36,
        DataSize = 40,
    }

    private enum FormatField
    {
        Channels = 2,
        Rate = 4,
        BytesPerSecond = 8,
        BlockAlign = 12,
        Bits = 14,
    }
}
