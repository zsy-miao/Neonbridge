// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;
using FiveStar.Loader.Pe;

namespace FiveStar.Loader.Loading;

/// <summary>Sets each section's page protection from its characteristics; the headers become read-only.</summary>
internal static unsafe class SectionProtector
{
    public static void Apply(LoadedImage image, LoaderLog log)
    {
        uint previous;
        _ = Kernel32.VirtualProtect(image.Base, image.Pe.SizeOfHeaders, Win32Constants.PageReadOnly, &previous);

        foreach (var section in image.Pe.Sections)
        {
            Protect(image, section, log);
        }

        _ = Kernel32.FlushInstructionCache(Kernel32.GetCurrentProcess(), image.Base, image.Size);
    }

    public static uint ProtectionFor(SectionCharacteristics characteristics)
    {
        bool execute = (characteristics & SectionCharacteristics.MemoryExecute) != 0;
        bool read = (characteristics & SectionCharacteristics.MemoryRead) != 0;
        bool write = (characteristics & SectionCharacteristics.MemoryWrite) != 0;

        if (execute)
        {
            return ExecutableProtection(read, write);
        }

        if (write)
        {
            return Win32Constants.PageReadWrite;
        }

        if (read)
        {
            return Win32Constants.PageReadOnly;
        }

        return Win32Constants.PageNoAccess;
    }

    private static uint ExecutableProtection(bool read, bool write)
    {
        if (write)
        {
            return Win32Constants.PageExecuteReadWrite;
        }

        if (read)
        {
            return Win32Constants.PageExecuteRead;
        }

        return Win32Constants.PageExecute;
    }

    private static void Protect(LoadedImage image, PeSection section, LoaderLog log)
    {
        uint size = Math.Max(section.VirtualSize, section.RawSize);

        if (size == 0)
        {
            return;
        }

        uint protection = ProtectionFor(section.Characteristics);
        uint previous;

        if (Kernel32.VirtualProtect(image.Va(section.VirtualAddress), size, protection, &previous) == 0)
        {
            log.Warn($"{image.Name}: VirtualProtect({section.Name}) failed: {Marshal.GetLastPInvokeError()}");
        }

        log.Detail($"  section {section.Name.PadRight(PeFormat.SectionNameSize)} +0x{section.VirtualAddress:x6} size 0x{size:x6} -> protect 0x{protection:x2}");
    }
}
