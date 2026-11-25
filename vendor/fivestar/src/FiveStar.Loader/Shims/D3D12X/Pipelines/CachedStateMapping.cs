// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;
using FiveStar.Shaders.Cache;

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>
/// A pipeline-cache record's decoded state (docs/pipeline-cache.md) as desktop descs. The decoder yields D3D12 enum
/// values, with 0 where the hardware value has none; those become the D3D12 defaults so the runtime accepts the desc.
/// The output formats are the record's own (its 0x0C entry); the register guesses are only a fallback without one.
/// </summary>
internal static class CachedStateMapping
{
    /// <summary>DXGI_FORMAT_D32_FLOAT: the fallback depth format of depth-only pipelines.</summary>
    public const int DepthFormat = 40;

    /// <summary>DXGI_FORMAT_D32_FLOAT_S8X24_UINT: the fallback depth format of pipelines that use stencil.</summary>
    public const int DepthStencilFormat = 20;

    /// <summary>DXGI_SAMPLE_DESC.Count of a pipeline that does not multisample.</summary>
    private const int SingleSample = 1;

    public static GraphicsPipelineDescription Describe(
        FixedFunctionState state, PipelineTargets? targets, nint gameRootSignature, IReadOnlyList<ShaderSource> shaders, IReadOnlyList<VertexElement> layout)
    {
        var topology = TopologyTypes.Desktop(state.TopologyType);
        var description = GraphicsPipelineDescription.Defaults(gameRootSignature) with
        {
            Shaders = shaders,
            Blend = Blend(state),
            Rasterizer = Rasterizer(state.Rasterizer, state.ForcedSampleCount),
            DepthStencil = DepthStencil(state.DepthStencil),
            InputLayout = layout,
            TopologyType = topology,
            RenderTargetFormats = Targets(state),
            DepthStencilFormat = Depth(state.DepthStencil),
        };

        if (targets is null)
        {
            return description;
        }

        return description with
        {
            RenderTargetFormats = targets.RenderTargetFormats,
            DepthStencilFormat = DepthFormats.Desktop(targets.DepthStencilFormat),
            SampleCount = Or(targets.SampleCount, SingleSample),
            SampleMask = targets.SampleMask,
        };
    }

    private static byte[] Blend(FixedFunctionState state)
    {
        var blend = DesktopStateBytes.DefaultBlend();
        DesktopStateBytes.Write(blend, DesktopStateBytes.IndependentBlendField, true);

        for (var slot = 0; slot < state.Blend.Count && slot < PipelineLayout.RenderTargetSlots; slot++)
        {
            var target = state.Blend[slot];
            var mask = (byte)target.WriteMask;

            if (!target.Enable)
            {
                DesktopStateBytes.WriteTarget(blend, slot, false, Off(), mask);

                continue;
            }

            int[] factors = [target.SrcBlend, target.DestBlend, target.BlendOp, target.SrcBlendAlpha, target.DestBlendAlpha, target.BlendOpAlpha];
            DesktopStateBytes.WriteTarget(blend, slot, true, factors, mask);
        }

        return blend;
    }

    private static int[] Off()
    {
        return
        [
            DesktopStateBytes.BlendOne, DesktopStateBytes.BlendZero, DesktopStateBytes.BlendOpAdd,
            DesktopStateBytes.BlendOne, DesktopStateBytes.BlendZero, DesktopStateBytes.BlendOpAdd,
        ];
    }

    private static byte[] Rasterizer(RasterizerState state, int forcedSampleCount)
    {
        var rasterizer = DesktopStateBytes.DefaultRasterizer();
        BinaryPrimitives.WriteInt32LittleEndian(rasterizer.AsSpan(DesktopStateBytes.FillModeField), Or(state.FillMode, DesktopStateBytes.FillSolid));
        BinaryPrimitives.WriteInt32LittleEndian(rasterizer.AsSpan(DesktopStateBytes.CullModeField), Or(state.CullMode, DesktopStateBytes.CullNone));
        DesktopStateBytes.Write(rasterizer, DesktopStateBytes.FrontCounterClockwiseField, state.FrontCounterClockwise);

        BinaryPrimitives.WriteInt32LittleEndian(rasterizer.AsSpan(DesktopStateBytes.DepthBiasField), state.DepthBias);
        BinaryPrimitives.WriteSingleLittleEndian(rasterizer.AsSpan(DesktopStateBytes.DepthBiasClampField), state.DepthBiasClamp);
        BinaryPrimitives.WriteSingleLittleEndian(rasterizer.AsSpan(DesktopStateBytes.SlopeScaledDepthBiasField), state.SlopeScaledDepthBias);
        DesktopStateBytes.Write(rasterizer, DesktopStateBytes.DepthClipField, state.DepthClipEnable);
        BinaryPrimitives.WriteInt32LittleEndian(rasterizer.AsSpan(DesktopStateBytes.ForcedSampleCountField), forcedSampleCount);

        return rasterizer;
    }

    private static byte[] DepthStencil(DepthStencilState state)
    {
        var depth = DesktopStateBytes.DefaultDepthStencil();
        var write = 0;

        if (state.DepthWrite)
        {
            write = DesktopStateBytes.DepthWriteAll;
        }

        DesktopStateBytes.Write(depth, DesktopStateBytes.DepthEnableField, state.DepthEnable);
        BinaryPrimitives.WriteInt32LittleEndian(depth.AsSpan(DesktopStateBytes.DepthWriteMaskField), write);
        BinaryPrimitives.WriteInt32LittleEndian(depth.AsSpan(DesktopStateBytes.DepthFuncField), Or(state.DepthFunc, DesktopStateBytes.ComparisonAlways));
        DesktopStateBytes.Write(depth, DesktopStateBytes.StencilEnableField, state.StencilEnable);
        depth[DesktopStateBytes.StencilReadMaskField] = (byte)state.StencilReadMask;
        depth[DesktopStateBytes.StencilWriteMaskField] = (byte)state.StencilWriteMask;

        DesktopStateBytes.WriteFace(depth, DesktopStateBytes.FrontFaceField, Face(state.Front));
        DesktopStateBytes.WriteFace(depth, DesktopStateBytes.BackFaceField, Face(state.Back));

        return depth;
    }

    private static int[] Face(StencilFace face)
    {
        var keep = DesktopStateBytes.StencilKeep;

        return [Or(face.FailOp, keep), Or(face.DepthFailOp, keep), Or(face.PassOp, keep), Or(face.Func, DesktopStateBytes.ComparisonAlways)];
    }

    private static int[] Targets(FixedFunctionState state)
    {
        var formats = new int[state.RenderTargetCount];

        foreach (var target in state.RenderTargets)
        {
            formats[target.Index] = target.Format;
        }

        return formats;
    }

    private static int Depth(DepthStencilState state)
    {
        if (!state.UsesDepthTarget)
        {
            return 0;
        }

        if (state.StencilEnable)
        {
            return DepthStencilFormat;
        }

        return DepthFormat;
    }

    /// <summary>The value, or the fallback when the decoder found no D3D12 equivalent (0).</summary>
    private static int Or(int value, int fallback)
    {
        if (value == 0)
        {
            return fallback;
        }

        return value;
    }
}
