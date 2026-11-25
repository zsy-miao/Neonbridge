// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Native;

/// <summary>Names addresses inside modules the OS loaded, for logs.</summary>
internal static unsafe class SystemModules
{
    /// <summary>What <see cref="Describe"/> returns for an address outside every OS-loaded module.</summary>
    public const string Unknown = "?";

    /// <summary>GetProcAddress by name, or by ordinal when <paramref name="name"/> is null.</summary>
    public static nint Export(nint module, string? name, uint ordinal)
    {
        if (name is null)
        {
            return Kernel32.GetProcAddress(module, (byte*)ordinal);
        }

        byte[] ansi = System.Text.Encoding.ASCII.GetBytes(name + "\0");

        fixed (byte* pointer = ansi)
        {
            return Kernel32.GetProcAddress(module, pointer);
        }
    }

    /// <summary>"module.dll+0xoffset" for an address in an OS-loaded module, else <see cref="Unknown"/>.</summary>
    public static string Describe(nint address)
    {
        nint module;
        uint flags = Win32Constants.GetModuleHandleExFlagFromAddress | Win32Constants.GetModuleHandleExFlagUnchangedRefcount;

        if (Kernel32.GetModuleHandleExW(flags, (char*)address, &module) == 0)
        {
            return Unknown;
        }

        return $"{FileName(module)}+0x{(long)address - (long)module:x}";
    }

    /// <summary>The file name of an OS-loaded module ("ws2_32.dll"); empty for an unknown handle.</summary>
    public static string FileName(nint module)
    {
        char* buffer = stackalloc char[Win32Constants.PathBufferLength];
        uint length = Kernel32.GetModuleFileNameW(module, buffer, Win32Constants.PathBufferLength);

        return Path.GetFileName(new string(buffer, 0, (int)length));
    }
}
