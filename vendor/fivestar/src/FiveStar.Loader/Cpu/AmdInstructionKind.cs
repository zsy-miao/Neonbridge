// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Cpu;

/// <summary>The AMD-only instructions the handler emulates; the values index its counters.</summary>
internal enum AmdInstructionKind
{
    /// <summary>SSE4a EXTRQ, immediate (66 0F 78 /0 ib ib) or register (66 0F 79 /r) form.</summary>
    Extrq,

    /// <summary>SSE4a INSERTQ, immediate (F2 0F 78 /r ib ib) or register (F2 0F 79 /r) form.</summary>
    Insertq,

    /// <summary>CLZERO (0F 01 FC): zero the cache line at rAX.</summary>
    Clzero,

    /// <summary>MONITORX (0F 01 FA): arm the monitor at rAX.</summary>
    MonitorX,

    /// <summary>MWAITX (0F 01 FB): wait for a store to the monitored line, with an optional timer.</summary>
    MWaitX,

    /// <summary>CLWB (66 0F AE /6, memory operand): write a cache line back.</summary>
    Clwb,
}
