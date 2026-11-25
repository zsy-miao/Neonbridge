// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.IO.Pipes;
using System.Text;

namespace FiveStar.Loader.Diagnostics;

/// <summary>
/// An EventPipe session of this process written to a .nettrace file, through the runtime's diagnostics IPC (the
/// named pipe <c>dotnet-diagnostic-&lt;pid&gt;</c>, the protocol dotnet-trace speaks): CollectTracing2 opens the session
/// and its connection carries the trace; StopTracing on a second connection ends it, after the rundown that names the
/// methods. The runtime's own server thread serves both, so the process can trace itself.
/// </summary>
internal sealed class EventPipeSession : IDisposable
{
    /// <summary>The CPU sampler: every managed thread's stack once a millisecond.</summary>
    public const string SampleProfiler = "Microsoft-DotNETCore-SampleProfiler";

    /// <summary>The runtime's events: with <see cref="MethodKeywords"/> the methods compiled during the session.</summary>
    public const string Runtime = "Microsoft-Windows-DotNETRuntime";

    /// <summary>Loader, JIT and NGen (ReadyToRun) keywords: what names a code address.</summary>
    public const ulong MethodKeywords = 0x38;

    public const uint Verbose = 5;

    private const string PipePrefix = "dotnet-diagnostic-";
    private const int HeaderBytes = 20;
    private const byte EventPipeCommands = 0x02;
    private const byte StopTracing = 0x01;
    private const byte CollectTracing2 = 0x03;
    private const byte ServerCommands = 0xFF;
    private const byte Ok = 0x00;
    private const uint NetTraceFormat = 1;
    private const uint BufferMegabytes = 256;
    private const int ConnectMilliseconds = 5000;
    private const int CopyBytes = 1 << 16;

    private static readonly byte[] Magic = Encoding.ASCII.GetBytes("DOTNET_IPC_V1\0");

    private readonly NamedPipeClientStream _pipe;
    private readonly FileStream _file;
    private readonly Thread _copier;
    private readonly ulong _id;

    private EventPipeSession(NamedPipeClientStream pipe, FileStream file, ulong id)
    {
        _pipe = pipe;
        _file = file;
        _id = id;
        _copier = new Thread(Copy) { IsBackground = true, Name = "fivestar trace writer" };
        _copier.Start();
    }

    /// <summary>The session's trace bytes written so far.</summary>
    public long Bytes => _file.Length;

    /// <summary>Starts a session with <paramref name="providers"/> (name, keywords, level) writing to <paramref name="path"/>.</summary>
    public static EventPipeSession Start(string path, IReadOnlyList<(string Name, ulong Keywords, uint Level)> providers)
    {
        var pipe = Connect();

        try
        {
            pipe.Write(Message(CollectTracing2, CollectPayload(providers)));
            ulong id = ReadOk(pipe);

            return new EventPipeSession(pipe, new FileStream(path, FileMode.Create, FileAccess.Write, FileShare.Read), id);
        }
        catch
        {
            pipe.Dispose();

            throw;
        }
    }

    /// <summary>Stops the session and waits until the runtime has written its rundown and closed the trace.</summary>
    public void Stop()
    {
        using (var pipe = Connect())
        {
            pipe.Write(Message(StopTracing, BitConverter.GetBytes(_id)));
            _ = ReadOk(pipe);
        }

        _copier.Join();
    }

    public void Dispose()
    {
        _pipe.Dispose();
        _copier.Join();
        _file.Dispose();
    }

    private static NamedPipeClientStream Connect()
    {
        // Identification only: a server squatting on the pipe name learns who connected but cannot act as this process.
        var pipe = new NamedPipeClientStream(".", PipePrefix + Environment.ProcessId, PipeDirection.InOut, PipeOptions.None, System.Security.Principal.TokenImpersonationLevel.Identification);
        pipe.Connect(ConnectMilliseconds);

        return pipe;
    }

    private void Copy()
    {
        var buffer = new byte[CopyBytes];

        try
        {
            int read;

            while ((read = _pipe.Read(buffer, 0, buffer.Length)) > 0)
            {
                _file.Write(buffer, 0, read);
            }
        }
        catch (IOException)
        {
            // The pipe closed under the read: the trace ends there.
        }
        catch (ObjectDisposedException)
        {
            // Disposed before the runtime ended the session.
        }

        _file.Flush();
    }

    /// <summary>CollectTracing2: buffer size, format, rundown, then each provider's keywords, level, name and (no) filter.</summary>
    private static byte[] CollectPayload(IReadOnlyList<(string Name, ulong Keywords, uint Level)> providers)
    {
        using var stream = new MemoryStream();
        using var writer = new BinaryWriter(stream);
        writer.Write(BufferMegabytes);
        writer.Write(NetTraceFormat);
        writer.Write(true);
        writer.Write((uint)providers.Count);

        foreach (var (name, keywords, level) in providers)
        {
            writer.Write(keywords);
            writer.Write(level);
            writer.Write((uint)(name.Length + 1));
            writer.Write(Encoding.Unicode.GetBytes(name + "\0"));
            writer.Write(0u);
        }

        writer.Flush();

        return stream.ToArray();
    }

    private static byte[] Message(byte command, byte[] payload)
    {
        var message = new byte[HeaderBytes + payload.Length];
        Magic.CopyTo(message, 0);
        BitConverter.TryWriteBytes(message.AsSpan(Magic.Length), (ushort)message.Length);
        message[Magic.Length + sizeof(ushort)] = EventPipeCommands;
        message[Magic.Length + sizeof(ushort) + 1] = command;
        payload.CopyTo(message, HeaderBytes);

        return message;
    }

    /// <summary>The server's answer: OK with a session id, or an error with its HRESULT.</summary>
    private static ulong ReadOk(Stream pipe)
    {
        var header = new byte[HeaderBytes];
        pipe.ReadExactly(header);
        var payload = new byte[BitConverter.ToUInt16(header, Magic.Length) - HeaderBytes];
        pipe.ReadExactly(payload);
        byte commandSet = header[Magic.Length + sizeof(ushort)];
        byte command = header[Magic.Length + sizeof(ushort) + 1];

        if (commandSet != ServerCommands || command != Ok || payload.Length < sizeof(ulong))
        {
            uint error = 0;

            if (payload.Length >= sizeof(uint))
            {
                error = BitConverter.ToUInt32(payload);
            }

            throw new IOException($"diagnostics IPC refused the session: 0x{error:x8}");
        }

        return BitConverter.ToUInt64(payload);
    }
}
