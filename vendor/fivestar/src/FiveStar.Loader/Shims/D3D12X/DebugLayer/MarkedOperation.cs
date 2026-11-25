// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Shims.D3D12X.DebugLayer;

/// <summary>What <see cref="GpuBreadcrumbs"/> keeps of a marked operation.</summary>
[StructLayout(LayoutKind.Sequential)]
internal struct MarkedOperation
{
    /// <summary>The desktop pipeline set then, or a <see cref="RecordedCommand"/> for copies and barriers.</summary>
    public nint Pipeline;

    /// <summary>ExecuteIndirect: the console address of the first argument (0 when the buffer has none) and the signature wrapper.</summary>
    public ulong Address;
    public nint Signature;

    /// <summary>Dispatch: the group counts; draws: vertices or indices and instances; ExecuteIndirect: the max count.</summary>
    public uint X;
    public uint Y;
    public uint Z;
    public MarkedKind Kind;
}
