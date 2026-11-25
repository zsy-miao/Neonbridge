// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Native;

/// <summary>The COM runtime calls the audio endpoint probe makes.</summary>
internal static unsafe class Ole32
{
    /// <summary>COINIT_MULTITHREADED.</summary>
    public const uint MultiThreaded = 0;

    /// <summary>CLSCTX_ALL.</summary>
    public const uint AllContexts = 0x17;

    private const string Library = "ole32";

    [DllImport(Library)]
    public static extern int CoInitializeEx(nint reserved, uint flags);

    [DllImport(Library)]
    public static extern void CoUninitialize();

    [DllImport(Library)]
    public static extern int CoCreateInstance(Guid* clsid, nint outer, uint context, Guid* iid, nint* instance);

    [DllImport(Library)]
    public static extern void CoTaskMemFree(void* memory);
}
