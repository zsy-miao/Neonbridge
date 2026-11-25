// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Native;

/// <summary>
/// Reads the x64 PEB and the loader's module list. Offsets are those of the public ntdll symbols (PEB,
/// PEB_LDR_DATA, LDR_DATA_TABLE_ENTRY) and PROCESS_BASIC_INFORMATION in winternl.h.
/// </summary>
internal static unsafe class ProcessEnvironmentBlock
{
    /// <summary>PROCESSINFOCLASS ProcessBasicInformation (winternl.h).</summary>
    private const int ProcessBasicInformationClass = 0;

    /// <summary>sizeof(PROCESS_BASIC_INFORMATION) on x64.</summary>
    private const int BasicInformationSize = 48;

    /// <summary>PROCESS_BASIC_INFORMATION.PebBaseAddress.</summary>
    private const int PebBaseAddressOffset = 8;

    /// <summary>PEB.ImageBaseAddress.</summary>
    public const int ImageBaseAddressOffset = 0x10;

    /// <summary>PEB.Ldr.</summary>
    private const int LdrOffset = 0x18;

    /// <summary>PEB_LDR_DATA.InLoadOrderModuleList.</summary>
    private const int InLoadOrderModuleListOffset = 0x10;

    /// <summary>LDR_DATA_TABLE_ENTRY.DllBase.</summary>
    private const int DllBaseOffset = 0x30;

    /// <summary>LDR_DATA_TABLE_ENTRY.TlsIndex.</summary>
    private const int TlsIndexOffset = 0x6E;

    /// <summary>Address of this process's PEB, or 0 when the query fails.</summary>
    public static nint Address()
    {
        byte* information = stackalloc byte[BasicInformationSize];
        int length;

        if (Ntdll.NtQueryInformationProcess(Kernel32.GetCurrentProcess(), ProcessBasicInformationClass, information, BasicInformationSize, &length) != 0)
        {
            return 0;
        }

        return *(nint*)(information + PebBaseAddressOffset);
    }

    /// <summary>LDR_DATA_TABLE_ENTRY.TlsIndex of a module the OS loaded; 0 when it is not in the list.</summary>
    public static ushort LoaderTlsIndexOf(nint module)
    {
        byte* peb = (byte*)Address();

        if (peb == null)
        {
            return 0;
        }

        byte* head = *(byte**)(peb + LdrOffset) + InLoadOrderModuleListOffset;

        for (byte* entry = *(byte**)head; entry != head; entry = *(byte**)entry)
        {
            if (*(nint*)(entry + DllBaseOffset) == module)
            {
                return *(ushort*)(entry + TlsIndexOffset);
            }
        }

        return 0;
    }
}
