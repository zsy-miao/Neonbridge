// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Shims.D3D12X;

/// <summary>
/// A fence wrapper: <see cref="XboxObject"/> plus the cell fence slot 12 hands the game, which reads the fence's
/// completed value from memory as on the console (<see cref="FenceValueMirror"/> keeps it current).
/// </summary>
[StructLayout(LayoutKind.Sequential)]
internal struct XboxFenceObject
{
    public XboxObject Header;
    public ulong CompletedValue;

    /// <summary>The last value a queue Signal set, and when (Stopwatch ticks); for the mirror's latency count.</summary>
    public ulong SignaledValue;
    public long SignaledAt;
}
