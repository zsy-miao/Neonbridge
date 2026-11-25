// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Shims.D3D12X;

/// <summary>D3D12_COMMAND_SIGNATURE_DESC (d3d12.h): byte stride, argument count and array, node mask.</summary>
[StructLayout(LayoutKind.Sequential)]
internal unsafe struct CommandSignatureDescription
{
    public uint ByteStride;
    public uint ArgumentCount;
    public IndirectArgument* Arguments;
    public uint NodeMask;
}
