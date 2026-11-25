// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Debugger.Native;

/// <summary>Reads and writes the debuggee's memory.</summary>
internal static unsafe class RemoteMemory
{
    public static bool TryRead<T>(nint process, ulong address, out T value)
        where T : unmanaged
    {
        T result;
        nuint read;
        bool ok = Kernel32.ReadProcessMemory(process, address, &result, (nuint)sizeof(T), &read) != 0 && read == (nuint)sizeof(T);
        value = result;

        return ok;
    }

    public static bool TryWrite<T>(nint process, ulong address, T value)
        where T : unmanaged
    {
        nuint written;

        return Kernel32.WriteProcessMemory(process, address, &value, (nuint)sizeof(T), &written) != 0 && written == (nuint)sizeof(T);
    }

    /// <summary>Reads as much of <paramref name="buffer"/> as is readable; returns the number of bytes read.</summary>
    public static int Read(nint process, ulong address, Span<byte> buffer)
    {
        nuint read = 0;

        fixed (byte* data = buffer)
        {
            _ = Kernel32.ReadProcessMemory(process, address, data, (nuint)buffer.Length, &read);
        }

        return (int)read;
    }
}
