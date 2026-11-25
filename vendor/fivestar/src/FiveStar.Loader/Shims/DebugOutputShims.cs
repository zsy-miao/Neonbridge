// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims;

/// <summary>
/// OutputDebugString from mapped code: the text goes to the loader log (the first <see cref="MaxLogged"/> messages),
/// then on to the debugger as usual. Retail builds still print some start-up and error messages this way.
/// </summary>
internal static unsafe partial class DebugOutputShims
{
    private const int MaxLogged = 4096;
    private const int MaxLength = 1024;

    private static int _logged;

    [Shim("*", "OutputDebugStringA")]
    [UnmanagedCallersOnly]
    private static void OutputDebugStringA(byte* text)
    {
        try
        {
            if (text != null && Interlocked.Increment(ref _logged) <= MaxLogged)
            {
                Write(Marshal.PtrToStringUTF8((nint)text));
            }

            Kernel32.OutputDebugStringA(text);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "DebugOutputShims.OutputDebugStringA");
        }
    }

    [Shim("*", "OutputDebugStringW")]
    [UnmanagedCallersOnly]
    private static void OutputDebugStringW(char* text)
    {
        try
        {
            if (text != null && Interlocked.Increment(ref _logged) <= MaxLogged)
            {
                Write(new string(text));
            }

            Kernel32.OutputDebugStringW(text);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "DebugOutputShims.OutputDebugStringW");
        }
    }

    private static void Write(string? text)
    {
        string line = (text ?? string.Empty).TrimEnd('\r', '\n');

        if (line.Length > MaxLength)
        {
            line = line[..MaxLength];
        }

        ShimContext.Runtime.Log.Info($"  [debug] {line}");
    }
}
