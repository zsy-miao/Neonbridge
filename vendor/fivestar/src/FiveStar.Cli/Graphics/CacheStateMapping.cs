// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Graphics.Pipelines;
using FiveStar.Shaders.Cache;using Vortice.Direct3D12;
using Vortice.DXGI;

namespace FiveStar.Cli.Graphics;

/// <summary>
/// The pipeline cache's decoded state and input layout as D3D12 descriptions. The cache stores D3D12 enum values, so
/// they are used as they are. A translated vertex shader names its fetched attributes <c>ATTR0..n</c> in input-layout
/// order, so element n of the cache layout becomes semantic ATTR index n with the cache's format, slot and offset.
/// </summary>
public static class CacheStateMapping
{
    public const string AttributeSemantic = "ATTR";

    private const int TargetSlots = 8;

    public static FixedFunctionDescription Describe(FixedFunctionState state)
    {
        return new FixedFunctionDescription(Blend(state), Rasterizer(state.Rasterizer), DepthStencil(state.DepthStencil));
    }

    /// <summary>
    /// The cache's state with a depth test: the pipeline's own when it tests depth, otherwise (the immediate-mode passes)
    /// depth test and write in the main passes' reverse-Z convention.
    /// </summary>
    public static (FixedFunctionDescription State, DepthConvention Depth) DepthTested(FixedFunctionState state)
    {
        var described = Describe(state);
        var depth = described.DepthStencil;

        if (!state.DepthStencil.DepthEnable)
        {
            depth.DepthEnable = true;
            depth.DepthWriteMask = DepthWriteMask.All;
            depth.DepthFunc = DepthConvention.Reverse.Test;
        }

        return (described with { DepthStencil = depth }, DepthConvention.Of(depth.DepthFunc));
    }

    public static IReadOnlyList<InputElementDescription> InputLayout(IReadOnlyList<InputElement> elements)
    {
        var layout = new List<InputElementDescription>();

        for (var index = 0; index < elements.Count; index++)
        {
            var element = elements[index];
            layout.Add(new InputElementDescription(
                AttributeSemantic,
                (uint)index,
                (Format)element.Format,
                (uint)element.Offset,
                (uint)element.InputSlot,
                (InputClassification)element.Classification,
                (uint)element.StepRate));
        }

        return layout;
    }

    /// <summary>The DXGI formats of the render targets the pixel shader writes (the cache's format guesses).</summary>
    public static IReadOnlyList<Format> Targets(FixedFunctionState state)
    {
        var formats = new Format[state.RenderTargetCount];

        foreach (var target in state.RenderTargets)
        {
            formats[target.Index] = (Format)target.Format;
        }

        return formats;
    }

    private static BlendDescription Blend(FixedFunctionState state)
    {
        var blend = new BlendDescription { AlphaToCoverageEnable = false, IndependentBlendEnable = true };

        for (var slot = 0; slot < TargetSlots; slot++)
        {
            var target = state.Blend[slot];
            blend.RenderTarget[slot] = new RenderTargetBlendDescription(
                target.Enable,
                false,
                (Vortice.Direct3D12.Blend)target.SrcBlend,
                (Vortice.Direct3D12.Blend)target.DestBlend,
                (BlendOperation)target.BlendOp,
                (Vortice.Direct3D12.Blend)target.SrcBlendAlpha,
                (Vortice.Direct3D12.Blend)target.DestBlendAlpha,
                (BlendOperation)target.BlendOpAlpha,
                LogicOp.Noop,
                (ColorWriteEnable)target.WriteMask);
        }

        return blend;
    }

    private static RasterizerDescription Rasterizer(RasterizerState state)
    {
        return new RasterizerDescription
        {
            FillMode = (FillMode)state.FillMode,
            CullMode = (CullMode)state.CullMode,
            FrontCounterClockwise = state.FrontCounterClockwise,
            DepthBias = state.DepthBias,
            DepthBiasClamp = state.DepthBiasClamp,
            SlopeScaledDepthBias = state.SlopeScaledDepthBias,
            DepthClipEnable = state.DepthClipEnable,
        };
    }

    private static DepthStencilDescription DepthStencil(DepthStencilState state)
    {
        var write = DepthWriteMask.Zero;

        if (state.DepthWrite)
        {
            write = DepthWriteMask.All;
        }

        return new DepthStencilDescription
        {
            DepthEnable = state.DepthEnable,
            DepthWriteMask = write,
            DepthFunc = (ComparisonFunction)state.DepthFunc,
            StencilEnable = state.StencilEnable,
            StencilReadMask = (byte)state.StencilReadMask,
            StencilWriteMask = (byte)state.StencilWriteMask,
            FrontFace = Face(state.Front),
            BackFace = Face(state.Back),
        };
    }

    private static DepthStencilOperationDescription Face(StencilFace face)
    {
        return new DepthStencilOperationDescription
        {
            StencilFailOp = (StencilOperation)face.FailOp,
            StencilDepthFailOp = (StencilOperation)face.DepthFailOp,
            StencilPassOp = (StencilOperation)face.PassOp,
            StencilFunc = (ComparisonFunction)face.Func,
        };
    }
}
