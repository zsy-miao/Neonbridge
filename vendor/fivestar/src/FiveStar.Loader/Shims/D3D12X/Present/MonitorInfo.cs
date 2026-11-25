// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Shims.D3D12X.Present;

/// <summary>MONITORINFO (winuser.h): its size, the monitor's rectangle, its work area (without the taskbar), flags.</summary>
[StructLayout(LayoutKind.Sequential)]
internal struct MonitorInfo
{
    public uint Size;
    public PlaneRect Monitor;
    public PlaneRect Work;
    public uint Flags;
}
