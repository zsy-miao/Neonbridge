// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Native;

/// <summary>OVERLAPPED (minwinbase.h) for an overlapped file read: the status words, the file offset and the event.</summary>
[StructLayout(LayoutKind.Sequential)]
internal struct FileOverlapped
{
    public nuint Internal;
    public nuint InternalHigh;
    public uint OffsetLow;
    public uint OffsetHigh;
    public nint Event;
}
