// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Shims.D3D12X;

/// <summary>
/// The head of every object the D3D12.X shim hands the game (our own layout): the D3D12.X vtable, then the desktop
/// object its forwarding slots call, at <see cref="D3D12XLayout.InnerOffset"/>. Some interfaces allocate more
/// (<see cref="XboxInterface.ObjectSize"/>): the command list for the fields the game's inline recording reads.
/// </summary>
[StructLayout(LayoutKind.Sequential)]
internal struct XboxObject
{
    public nint Vtable;
    public nint Inner;
}
