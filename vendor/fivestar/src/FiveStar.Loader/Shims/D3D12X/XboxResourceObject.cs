// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Shims.D3D12X;

/// <summary>
/// A resource wrapper: <see cref="XboxObject"/> plus the console GPU address the game chose for it (device slot 133),
/// which GetGPUVirtualAddress returns instead of the desktop one (0 for resources the desktop placed), and the title
/// memory allocated for a slot-127 resource (<see cref="XboxCommittedResources"/>), and the D3D12_RESOURCE_DIMENSION
/// and legacy state the desktop resource rests in between command lists, and its desktop flags (<see cref="States.ResourceStates"/>;
/// dimension 0, UNKNOWN, when the shim did not create it).
/// </summary>
[StructLayout(LayoutKind.Sequential)]
internal struct XboxResourceObject
{
    public XboxObject Header;
    public ulong GameAddress;
    public nint TitleMemory;
    public int Dimension;
    public int HomeState;

    /// <summary>The desktop D3D12_RESOURCE_FLAGS: which write states a barrier may move it to.</summary>
    public int Flags;
}
