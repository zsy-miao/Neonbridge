// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>
/// Fixed-function combinations the console takes and the desktop runtime refuses (E_INVALIDARG), rewritten to the
/// nearest desktop form before a pipeline is created:
/// <list type="bullet">
/// <item>dual-source blending (a SRC1 factor) on target 0 with independent blending on: the game repeats target 0's
/// blend on the other seven, the runtime allows dual-source factors only when target 0 is the one blend
/// (debug-layer errors 112, 113, 115). Independent blending is turned off and targets 1 to 7 get the default blend
/// (one render target is all dual-source blending can write);</item>
/// <item>a forced sample count with depth or stencil on (error 672): the forced count is dropped;</item>
/// <item>a NaN depth-bias clamp or slope scale (CreateRasterizerState: invalid value): it becomes 0, no clamp and no
/// slope bias.</item>
/// </list>
/// </summary>
internal static class DesktopStateRules
{
    // D3D12_BLEND_SRC1_COLOR, _INV_SRC1_COLOR, _SRC1_ALPHA, _INV_SRC1_ALPHA (d3d12.h).
    private const int FirstDualSourceBlend = 16;
    private const int LastDualSourceBlend = 19;

    /// <summary>The blend desc the desktop accepts (a copy when it changes).</summary>
    public static byte[] Blend(byte[] blend)
    {
        if (!UsesDualSource(blend))
        {
            return blend;
        }

        var desktop = DesktopStateBytes.DefaultBlend();
        int throughTarget0 = DesktopStateBytes.TargetsField + DesktopStateBytes.TargetSize;
        blend.AsSpan(0, throughTarget0).CopyTo(desktop);
        DesktopStateBytes.Write(desktop, DesktopStateBytes.IndependentBlendField, false);

        return desktop;
    }

    /// <summary>The rasterizer desc the desktop accepts with <paramref name="depthStencil"/> (a copy when it changes).</summary>
    public static byte[] Rasterizer(byte[] rasterizer, byte[] depthStencil)
    {
        bool forced = BinaryPrimitives.ReadUInt32LittleEndian(rasterizer.AsSpan(DesktopStateBytes.ForcedSampleCountField)) != 0;
        bool depth = BinaryPrimitives.ReadInt32LittleEndian(depthStencil.AsSpan(DesktopStateBytes.DepthEnableField)) != 0;
        bool stencil = BinaryPrimitives.ReadInt32LittleEndian(depthStencil.AsSpan(DesktopStateBytes.StencilEnableField)) != 0;
        bool dropForced = forced && (depth || stencil);
        bool nanBias = IsNaN(rasterizer, DesktopStateBytes.DepthBiasClampField) || IsNaN(rasterizer, DesktopStateBytes.SlopeScaledDepthBiasField);

        if (!dropForced && !nanBias)
        {
            return rasterizer;
        }

        var desktop = (byte[])rasterizer.Clone();

        if (dropForced)
        {
            BinaryPrimitives.WriteUInt32LittleEndian(desktop.AsSpan(DesktopStateBytes.ForcedSampleCountField), 0);
        }

        foreach (var field in (int[])[DesktopStateBytes.DepthBiasClampField, DesktopStateBytes.SlopeScaledDepthBiasField])
        {
            if (IsNaN(desktop, field))
            {
                BinaryPrimitives.WriteSingleLittleEndian(desktop.AsSpan(field), 0);
            }
        }

        return desktop;
    }

    private static bool IsNaN(byte[] desc, int field)
    {
        return float.IsNaN(BinaryPrimitives.ReadSingleLittleEndian(desc.AsSpan(field)));
    }

    private static bool UsesDualSource(byte[] blend)
    {
        int[] fields = [DesktopStateBytes.SrcBlendField, DesktopStateBytes.DestBlendField, DesktopStateBytes.SrcBlendAlphaField, DesktopStateBytes.DestBlendAlphaField];

        foreach (int field in fields)
        {
            int factor = BinaryPrimitives.ReadInt32LittleEndian(blend.AsSpan(DesktopStateBytes.TargetsField + field));

            if (factor is >= FirstDualSourceBlend and <= LastDualSourceBlend)
            {
                return true;
            }
        }

        return false;
    }
}
