// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.Apu;

/// <summary>
/// The acphal.dll structures the game hands over, inferred from its audio start-up (docs/audio-runtime.md): the
/// SHAPE_CONTEXTS block AcpHalAllocateShapeContexts fills and the IAcpHal slots the game calls.
/// </summary>
internal static class AcpHalLayout
{
    /// <summary>SHAPE_CONTEXTS: six u32 context counts; the game sets only the fifth (350), the XMA decoders.</summary>
    public const int ContextKinds = 6;

    public const int XmaKind = 4;

    /// <summary>SHAPE_CONTEXTS: the context array pointers follow the counts, one per kind.</summary>
    public const int PointersOffset = 0x18;

    /// <summary>Bytes of one XMA context; the game zeroes count * 64 bytes of the array.</summary>
    public const int XmaContextBytes = 0x40;

    /// <summary>The alignment of the context array (hardware-visible memory; the console value is unknown).</summary>
    public const uint ContextAlignment = 0x100;

    /// <summary>IAcpHal slots: Initialize(this, u32, u32), Shutdown(this), SendMessage(this, type, cookie, timeout, payload*, flags), two unknown, Release(this).</summary>
    public const int InitializeSlot = 0;
    public const int ShutdownSlot = 1;
    public const int SendMessageSlot = 2;
    public const int UnknownSlot3 = 3;
    public const int UnknownSlot4 = 4;
    public const int ReleaseSlot = 5;
    public const int Slots = 6;
}
