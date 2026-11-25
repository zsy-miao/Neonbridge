// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using System.Text;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>UTF-8 strings across the GDK boundary.</summary>
internal static unsafe class Utf8Text
{
    public static string? Read(nint text)
    {
        if (text == 0)
        {
            return null;
        }

        return Marshal.PtrToStringUTF8(text);
    }

    /// <summary>
    /// The GDK string-out convention (size, char* buffer, size_t* used): <paramref name="used"/> receives the size
    /// including the NUL; a missing or short buffer gives HRESULT_FROM_WIN32(ERROR_INSUFFICIENT_BUFFER).
    /// </summary>
    public static int Write(string text, nuint size, byte* buffer, nuint* used)
    {
        var bytes = Encoding.UTF8.GetBytes(text);
        nuint needed = (nuint)bytes.Length + 1;

        if (used != null)
        {
            *used = needed;
        }

        if (buffer == null || size < needed)
        {
            return HResult.NotSufficientBuffer;
        }

        bytes.CopyTo(new Span<byte>(buffer, bytes.Length));
        buffer[bytes.Length] = 0;

        return HResult.Ok;
    }

    /// <summary>A NUL-terminated UTF-8 copy, freed with <see cref="NativeMemory.Free(void*)"/>.</summary>
    public static byte* Allocate(string text)
    {
        var bytes = Encoding.UTF8.GetBytes(text);
        byte* copy = (byte*)NativeMemory.Alloc((nuint)bytes.Length + 1);
        bytes.CopyTo(new Span<byte>(copy, bytes.Length));
        copy[bytes.Length] = 0;

        return copy;
    }

    /// <summary>Bytes of the UTF-8 form plus the NUL.</summary>
    public static nuint SizeWithTerminator(string text)
    {
        return (nuint)Encoding.UTF8.GetByteCount(text) + 1;
    }
}
