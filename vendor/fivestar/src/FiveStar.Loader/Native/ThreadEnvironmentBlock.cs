// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Native;

/// <summary>x64 TEB offsets, addressed through gs (public ntdll symbols for TEB; winternl.h).</summary>
internal static class ThreadEnvironmentBlock
{
    /// <summary>TEB.ThreadLocalStoragePointer: the implicit-TLS block array compiled code reads as gs:[58h].</summary>
    public const int ThreadLocalStoragePointer = 0x58;

    /// <summary>TEB.LastErrorValue: what SetLastError writes.</summary>
    public const int LastErrorValue = 0x68;

    /// <summary>TEB.TlsSlots: the first 64 TlsAlloc slots.</summary>
    public const int TlsSlots = 0x1480;
    public const int TlsSlotCount = 64;
}
