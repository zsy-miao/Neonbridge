// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>
/// Zeroes the padding of the fixed-function descs: the game leaves stack bytes there (after each target's UINT8
/// RenderTargetWriteMask, after the stencil masks), which the driver ignores but which would give the same pipeline
/// another key in the disk cache (<see cref="DesktopStream.Key"/>).
/// </summary>
internal static class StatePadding
{
    // D3D12_RENDER_TARGET_BLEND_DESC: UINT8 RenderTargetWriteMask, then 3 bytes up to the next target.
    private const int WriteMaskPadding = 3;

    // D3D12_DEPTH_STENCIL_DESC1: UINT8 StencilReadMask @0x10, StencilWriteMask @0x11, 2 bytes up to FrontFace @0x14.
    private const int StencilMaskPadding = 2;

    /// <summary>A copy of a D3D12_BLEND_DESC with zero padding.</summary>
    public static byte[] Blend(byte[] blend)
    {
        var copy = (byte[])blend.Clone();

        for (var slot = 0; slot < PipelineLayout.RenderTargetSlots; slot++)
        {
            var padding = DesktopStateBytes.TargetsField + (slot * DesktopStateBytes.TargetSize) + DesktopStateBytes.WriteMaskField + 1;
            copy.AsSpan(padding, WriteMaskPadding).Clear();
        }

        return copy;
    }

    /// <summary>A copy of a D3D12_DEPTH_STENCIL_DESC1 with zero padding.</summary>
    public static byte[] DepthStencil(byte[] depthStencil)
    {
        var copy = (byte[])depthStencil.Clone();
        copy.AsSpan(DesktopStateBytes.StencilWriteMaskField + 1, StencilMaskPadding).Clear();

        return copy;
    }
}
