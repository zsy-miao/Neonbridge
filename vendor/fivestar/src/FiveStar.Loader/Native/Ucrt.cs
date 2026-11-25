// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Native;

/// <summary>The ucrtbase file functions the file shims forward to: the same instance mapped code binds to.</summary>
internal static unsafe class Ucrt
{
    private const string Library = "ucrtbase";

    [DllImport(Library, EntryPoint = "fopen")]
    public static extern nint FOpen(byte* name, byte* mode);

    [DllImport(Library, EntryPoint = "_wfopen")]
    public static extern nint WFOpen(char* name, char* mode);

    [DllImport(Library, EntryPoint = "_fsopen")]
    public static extern nint FsOpen(byte* name, byte* mode, int share);

    [DllImport(Library, EntryPoint = "_wfsopen")]
    public static extern nint WFsOpen(char* name, char* mode, int share);

    [DllImport(Library, EntryPoint = "_open")]
    public static extern int Open(byte* name, int flags, int mode);

    [DllImport(Library, EntryPoint = "_wopen")]
    public static extern int WOpen(char* name, int flags, int mode);
}
