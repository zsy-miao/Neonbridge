// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Debugger.Native;

/// <summary>The MINIDUMP_TYPE flags the debugger writes (minidumpapiset.h).</summary>
[Flags]
internal enum MinidumpType : uint
{
    Normal = 0,
    WithDataSegments = 0x1,
    WithFullMemory = 0x2,
    WithHandleData = 0x4,
    WithUnloadedModules = 0x20,
    WithIndirectlyReferencedMemory = 0x40,
    WithProcessThreadData = 0x100,
    WithFullMemoryInfo = 0x800,
    WithThreadInfo = 0x1000,
}
