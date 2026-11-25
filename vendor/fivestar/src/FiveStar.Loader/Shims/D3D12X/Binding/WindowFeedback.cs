// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Shims.D3D12X.DebugLayer;

namespace FiveStar.Loader.Shims.D3D12X.Binding;

/// <summary>
/// <c>--evict-windows</c>, the GPU's side: an address-map page that waits for its section window's heap holds the null
/// views and, in its offset field, <see cref="WaitTag"/> with the window's id. Programs made on demand define
/// FIVESTAR_WINDOW_FEEDBACK as <see cref="Base"/>, and their <c>gpu_page</c> (rdna_prelude) then stores 1 at
/// <see cref="Base"/> + id * 4 in the descriptor window when an access lands on such a page, so the loader learns
/// which windows the GPU wants (<see cref="WindowResidency"/>). The access itself still reads zeros.
/// </summary>
internal static class WindowFeedback
{
    /// <summary>Offset-field bit of a waiting page; the low 16 bits are its window's id.</summary>
    public const uint WaitTag = 0x80000000;

    /// <summary>Window ids: 1 to this, 0 is none.</summary>
    public const int MaxIds = 0xFFFF;

    /// <summary>The flags' window offset: past the store-trace ring, so both fit.</summary>
    public const uint Base = PoolStoreTrace.Bytes;

    /// <summary>Window bytes the flags need.</summary>
    public const uint Bytes = Base + ((MaxIds + 1) * sizeof(uint));

    /// <summary>Whether section windows are opened as the GPU asks for them and released when they are not used.</summary>
    public static bool Enabled { get; set; }

    /// <summary>The define a program made on demand starts with in this mode.</summary>
    public static string Header => $"#define FIVESTAR_WINDOW_FEEDBACK {Base}u\n";
}
