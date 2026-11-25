// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using Vortice.Direct3D12;
using Vortice.DXGI;

namespace FiveStar.Shaders.Tests.Warp;

/// <summary>Root signatures and pipeline states for the translator's bindless model.</summary>
public static class PipelineFactory
{
    private const int FallbackSpace = 9;
    private const int FallbackUavs = 3;

    /// <summary>The root signature with the user data at b0 bound as <paramref name="binding"/> says, then <paramref name="layout"/>'s parameters.</summary>
    public static ID3D12RootSignature RootSignature(ID3D12Device device, int rootConstants, RootLayout layout, UserDataBinding binding)
    {
        var flags = RootSignatureFlags.ConstantBufferViewShaderResourceViewUnorderedAccessViewHeapDirectlyIndexed
            | RootSignatureFlags.SamplerHeapDirectlyIndexed | RootSignatureFlags.AllowInputAssemblerInputLayout;
        var parameters = new List<RootParameter1> { UserData(rootConstants, binding) };

        if (layout == RootLayout.FallbackRoot)
        {
            for (uint register = 0; register < FallbackUavs; register++)
            {
                parameters.Add(new RootParameter1(RootParameterType.UnorderedAccessView, new RootDescriptor1(register, FallbackSpace), ShaderVisibility.All));
            }

            parameters.Add(new RootParameter1(RootParameterType.ShaderResourceView, new RootDescriptor1(0, FallbackSpace), ShaderVisibility.All));
        }
        else if (layout == RootLayout.FallbackTable)
        {
            var volatileFlags = DescriptorRangeFlags.DataVolatile | DescriptorRangeFlags.DescriptorsVolatile;
            DescriptorRange1[] ranges =
            [
                new(DescriptorRangeType.UnorderedAccessView, FallbackUavs, 0, FallbackSpace, 0, volatileFlags),
                new(DescriptorRangeType.ShaderResourceView, 1, 0, FallbackSpace, FallbackUavs, volatileFlags),
            ];
            parameters.Add(new RootParameter1(new RootDescriptorTable1(ranges), ShaderVisibility.All));
        }

        return device.CreateRootSignature(new RootSignatureDescription1(flags, [.. parameters]));
    }

    private static RootParameter1 UserData(int rootConstants, UserDataBinding binding)
    {
        if (binding == UserDataBinding.Buffer)
        {
            return new RootParameter1(RootParameterType.ConstantBufferView, new RootDescriptor1(0, 0, RootDescriptorFlags.DataVolatile), ShaderVisibility.All);
        }

        if (binding == UserDataBinding.UserConstants)
        {
            rootConstants = UserDataBindings.UserConstants;
        }

        return new RootParameter1(new RootConstants(0, 0, (uint)rootConstants), ShaderVisibility.All);
    }

    public static ID3D12PipelineState Compute(ID3D12Device device, ID3D12RootSignature root, byte[] shader)
    {
        return device.CreateComputePipelineState<ID3D12PipelineState>(new ComputePipelineStateDescription
        {
            RootSignature = root,
            ComputeShader = shader,
        });
    }

    /// <summary>VS (+ GS) + PS, no culling, no blending, no depth.</summary>
    public static ID3D12PipelineState Graphics(ID3D12Device device, ID3D12RootSignature root, GraphicsJob job, Format[] targets)
    {
        var topology = job.Points ? PrimitiveTopologyType.Point : PrimitiveTopologyType.Triangle;
        var description = new GraphicsPipelineStateDescription
        {
            RootSignature = root,
            VertexShader = job.VertexShader,
            PixelShader = job.PixelShader,
            PrimitiveTopologyType = topology,
            RenderTargetFormats = targets,
            DepthStencilFormat = Format.Unknown,
            SampleDescription = new SampleDescription(1, 0),
            RasterizerState = RasterizerDescription.CullNone,
            BlendState = NoBlending(),
            DepthStencilState = DepthStencilDescription.None,
            SampleMask = uint.MaxValue,
        };

        if (job.GeometryShader is not null)
        {
            description.GeometryShader = job.GeometryShader;
        }

        if (job.InputLayout.Count > 0)
        {
            description.InputLayout = new InputLayoutDescription([.. job.InputLayout.Select(Element)]);
        }

        return device.CreateGraphicsPipelineState<ID3D12PipelineState>(description);
    }

    /// <summary>Blending off on every target (integer targets cannot blend; Vortice's Opaque preset enables it).</summary>
    private static BlendDescription NoBlending()
    {
        var blend = new BlendDescription { AlphaToCoverageEnable = false, IndependentBlendEnable = false };

        for (var target = 0; target < 8; target++)
        {
            blend.RenderTarget[target] = new RenderTargetBlendDescription
            {
                BlendEnable = false,
                LogicOpEnable = false,
                SourceBlend = Blend.One,
                DestinationBlend = Blend.Zero,
                BlendOperation = BlendOperation.Add,
                SourceBlendAlpha = Blend.One,
                DestinationBlendAlpha = Blend.Zero,
                BlendOperationAlpha = BlendOperation.Add,
                LogicOp = LogicOp.Noop,
                RenderTargetWriteMask = ColorWriteEnable.All,
            };
        }

        return blend;
    }

    private static InputElementDescription Element(VertexElement attribute)
    {
        var classification = attribute.PerInstance ? InputClassification.PerInstanceData : InputClassification.PerVertexData;
        var stepRate = attribute.PerInstance ? 1u : 0u;

        return new InputElementDescription(
            attribute.Semantic, (uint)attribute.Index, attribute.Format, (uint)attribute.Offset, (uint)attribute.Slot, classification, stepRate);
    }
}
