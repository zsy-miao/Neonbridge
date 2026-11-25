// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Shims.D3D12X.Present;

/// <summary>DXGI_SWAP_CHAIN_DESC1 (dxgi1_2.h).</summary>
[StructLayout(LayoutKind.Sequential)]
internal struct SwapChainDescription
{
    public uint Width;
    public uint Height;
    public int Format;
    public int Stereo;
    public uint SampleCount;
    public uint SampleQuality;
    public uint BufferUsage;
    public uint BufferCount;
    public uint Scaling;
    public uint SwapEffect;
    public uint AlphaMode;
    public uint Flags;
}
