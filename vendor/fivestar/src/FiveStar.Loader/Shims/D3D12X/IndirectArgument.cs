// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Shims.D3D12X;

/// <summary>D3D12_INDIRECT_ARGUMENT_DESC (d3d12.h): the type and up to three dwords of its parameters.</summary>
[StructLayout(LayoutKind.Sequential)]
internal struct IndirectArgument
{
    /// <summary>D3D12_INDIRECT_ARGUMENT_TYPE_DISPATCH.</summary>
    public const int DispatchType = 2;

    /// <summary>
    /// A D3D12.X-only type with a 12-byte argument (signature built at 0x1424794da next to a desktop DISPATCH one),
    /// read as a dispatch (*inferred*).
    /// </summary>
    public const int XboxDispatchType = 0x69;

    public int Type;
    public uint First;
    public uint Second;
    public uint Third;
}
