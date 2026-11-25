// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Debugger.Native;

/// <summary>Reads facts of an image from its headers in the debuggee and of the file behind a debug event's handle.</summary>
internal static unsafe class RemoteImage
{
    /// <summary>IMAGE_DOS_HEADER.e_lfanew.</summary>
    private const ulong NewHeaderOffset = 0x3C;

    /// <summary>IMAGE_NT_HEADERS64.OptionalHeader.SizeOfImage: 4-byte signature, 20-byte file header, then 0x38 into the optional header.</summary>
    private const ulong SizeOfImageOffset = 0x50;

    /// <summary>GetFinalPathNameByHandle buffer, in characters.</summary>
    private const int PathCapacity = 1024;

    /// <summary>FILE_NAME_NORMALIZED | VOLUME_NAME_DOS.</summary>
    private const uint DosPath = 0;

    private const string LongPathPrefix = @"\\?\";

    /// <summary>SizeOfImage, or 0 when the headers cannot be read.</summary>
    public static ulong SizeOf(nint process, ulong imageBase)
    {
        if (!RemoteMemory.TryRead(process, imageBase + NewHeaderOffset, out uint newHeader))
        {
            return 0;
        }

        if (!RemoteMemory.TryRead(process, imageBase + newHeader + SizeOfImageOffset, out uint size))
        {
            return 0;
        }

        return size;
    }

    /// <summary>The path of the file and closes the handle (the debugger owns the image handles of debug events).</summary>
    public static string? TakePath(nint file)
    {
        if (file == 0)
        {
            return null;
        }

        char* buffer = stackalloc char[PathCapacity];
        uint length = Kernel32.GetFinalPathNameByHandle(file, buffer, PathCapacity, DosPath);
        _ = Kernel32.CloseHandle(file);

        if (length == 0 || length >= PathCapacity)
        {
            return null;
        }

        string path = new(buffer, 0, (int)length);

        if (path.StartsWith(LongPathPrefix, StringComparison.Ordinal))
        {
            return path[LongPathPrefix.Length..];
        }

        return path;
    }
}
