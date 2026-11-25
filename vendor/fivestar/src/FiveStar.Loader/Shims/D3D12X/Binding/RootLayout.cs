// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Shims.D3D12X.Binding;

/// <summary>
/// Where each root parameter of one of the game's root signatures reaches a program: its first user SGPR, counted from
/// the stage's first root SGPR (<see cref="UserData"/>), and how many SGPRs it takes. Native memory, so a command list
/// reads it while recording without touching managed objects.
/// </summary>
[StructLayout(LayoutKind.Sequential)]
internal unsafe struct RootLayout
{
    /// <summary>D3D12_MAX_ROOT_COST: no root signature has more parameters (d3d12.h).</summary>
    public const int MaxParameters = 64;

    public int Count;
    public fixed byte Kinds[MaxParameters];
    public fixed byte Offsets[MaxParameters];
    public fixed byte Sizes[MaxParameters];

    /// <summary>Descriptors a table parameter spans (the end of its last range), at most <see cref="DescriptorWindow.MaxTableRecords"/>.</summary>
    public fixed ushort Extents[MaxParameters];

    /// <summary>The user SGPRs all parameters take, packed in order.</summary>
    public readonly int Sgprs()
    {
        if (Count == 0)
        {
            return 0;
        }

        return Offsets[Count - 1] + Sizes[Count - 1];
    }

    /// <summary>The parameter's kind, or null past <see cref="Count"/>.</summary>
    public readonly RootParameterKind? Kind(uint parameter)
    {
        if (parameter >= (uint)Count)
        {
            return null;
        }

        return (RootParameterKind)Kinds[parameter];
    }
}
