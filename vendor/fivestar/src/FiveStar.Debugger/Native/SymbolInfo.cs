// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Debugger.Native;

/// <summary>
/// The fixed part of SYMBOL_INFOW (dbghelp.h), x64 layout: 0x58 bytes, with the UTF-16 name of up to MaxNameLen
/// characters at 0x54. The caller allocates the name buffer right behind it.
/// </summary>
[StructLayout(LayoutKind.Explicit, Size = 0x58)]
internal struct SymbolInfo
{
    /// <summary>SYMBOL_INFOW.Name.</summary>
    public const int NameOffset = 0x54;

    /// <summary>SYMFLAG_EXPORT: the name is the nearest export, not a symbol from a PDB.</summary>
    public const uint ExportFlag = 0x200;

    [FieldOffset(0x0)] public uint SizeOfStruct;
    [FieldOffset(0x28)] public uint Flags;
    [FieldOffset(0x4C)] public uint NameLength;
    [FieldOffset(0x50)] public uint MaxNameLength;
}
