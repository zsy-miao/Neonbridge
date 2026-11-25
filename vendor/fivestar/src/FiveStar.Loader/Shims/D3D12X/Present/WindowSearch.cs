// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Shims.D3D12X.Present;

/// <summary>The state of one <see cref="GameWindowFinder"/> enumeration, on the caller's stack.</summary>
[StructLayout(LayoutKind.Sequential)]
internal struct WindowSearch
{
    public uint ProcessId;
    public nint Console;
    public nint Best;
    public long BestArea;
}
