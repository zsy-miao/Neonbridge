// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Shims.D3D12X;

/// <summary>
/// The rectangle-list state of one command list, kept in the list's wrapper after the binding state: the desktop
/// pipeline the game set, the console topology last set and whether the pipeline's rectangle-list variant is bound
/// in its place. The wrapper starts zeroed and Reset zeroes it again.
/// </summary>
[StructLayout(LayoutKind.Sequential)]
internal unsafe struct RectangleListState
{
    /// <summary>Offset in the command-list wrapper, past <see cref="Binding.CommandListBindings"/>.</summary>
    public const int Offset = 0xF00;

    public nint Pipeline;
    public uint Topology;
    public int VariantBound;

    public static RectangleListState* Of(XboxObject* list)
    {
        return (RectangleListState*)((byte*)list + Offset);
    }
}
