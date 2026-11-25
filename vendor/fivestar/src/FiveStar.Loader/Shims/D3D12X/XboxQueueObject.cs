// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Shims.D3D12X;

/// <summary>
/// A command-queue wrapper: <see cref="XboxObject"/> plus a desktop fence the shim signals after each submission and
/// the last value signalled, which tell when a list submitted on this queue has finished (<see cref="ListSubmissions"/>),
/// and its number in <see cref="QueueProgress"/> (0 until its first call).
/// </summary>
[StructLayout(LayoutKind.Sequential)]
internal struct XboxQueueObject
{
    public XboxObject Header;
    public nint Fence;
    public ulong Signaled;
    public int Number;
}
