// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using Vortice.Direct3D12;
using Vortice.DXGI;

namespace FiveStar.Graphics.Pipelines;

/// <summary>
/// Creates compute, graphics and mesh pipeline states on one root signature: by default no culling, no blending, depth
/// test off, one sample. A rejected pipeline throws the driver's <c>SharpGenException</c>.
/// </summary>
public sealed class PipelineBuilder
{
    // D3D12_SIMULTANEOUS_RENDER_TARGET_COUNT.
    private const int TargetSlots = 8;

    private readonly ID3D12Device _device;
    private readonly ID3D12RootSignature _root;

    public PipelineBuilder(ID3D12Device device, ID3D12RootSignature root)
    {
        _device = device;
        _root = root;
    }

    public ID3D12PipelineState Compute(byte[] shader)
    {
        return _device.CreateComputePipelineState<ID3D12PipelineState>(new ComputePipelineStateDescription
        {
            RootSignature = _root,
            ComputeShader = shader,
        });
    }

    public ID3D12PipelineState Graphics(GraphicsProgram program)
    {
        return Graphics(program, new FixedFunctionDescription(NoBlending(), RasterizerDescription.CullNone, DepthStencilDescription.None));
    }

    /// <summary>A graphics pipeline with the given blend, rasterizer and depth-stencil state.</summary>
    public ID3D12PipelineState Graphics(GraphicsProgram program, FixedFunctionDescription state)
    {
        var description = new GraphicsPipelineStateDescription
        {
            RootSignature = _root,
            VertexShader = program.VertexShader,
            PrimitiveTopologyType = program.Topology,
            RenderTargetFormats = [.. program.Targets],
            DepthStencilFormat = program.DepthFormat,
            SampleDescription = new SampleDescription(1, 0),
            RasterizerState = state.Rasterizer,
            BlendState = state.Blend,
            DepthStencilState = state.DepthStencil,
            SampleMask = uint.MaxValue,
        };

        if (program.GeometryShader is not null)
        {
            description.GeometryShader = program.GeometryShader;
        }

        if (program.PixelShader is not null)
        {
            description.PixelShader = program.PixelShader;
        }

        if (program.InputLayout.Count > 0)
        {
            description.InputLayout = new InputLayoutDescription([.. program.InputLayout]);
        }

        return _device.CreateGraphicsPipelineState<ID3D12PipelineState>(description);
    }

    public ID3D12PipelineState Mesh(MeshProgram program)
    {
        using var stream = new PipelineSubobjects();
        stream.AddPointer(PipelineStateSubObjectType.RootSignature, _root.NativePointer);
        stream.AddShader(PipelineStateSubObjectType.MeshShader, program.MeshShader);

        if (program.PixelShader is not null)
        {
            stream.AddShader(PipelineStateSubObjectType.PixelShader, program.PixelShader);
        }

        stream.Add(PipelineStateSubObjectType.Rasterizer, RasterizerDescription.CullNone);
        stream.Add(PipelineStateSubObjectType.DepthStencil, DepthStencilDescription.None);
        stream.AddWords(PipelineStateSubObjectType.RenderTargetFormats, TargetFormatArray(program.Targets));
        stream.AddWords(PipelineStateSubObjectType.DepthStencilFormat, [(int)program.DepthFormat]);
        stream.Add(PipelineStateSubObjectType.SampleDescription, new SampleDescription(1, 0));

        return stream.Create(_device);
    }

    /// <summary>D3D12_RT_FORMAT_ARRAY: eight DXGI formats, then the target count.</summary>
    private static int[] TargetFormatArray(IReadOnlyList<Format> targets)
    {
        var words = new int[TargetSlots + 1];

        for (var slot = 0; slot < targets.Count; slot++)
        {
            words[slot] = (int)targets[slot];
        }

        words[TargetSlots] = targets.Count;

        return words;
    }

    /// <summary>Blending off on every target (integer targets cannot blend).</summary>
    private static BlendDescription NoBlending()
    {
        var blend = new BlendDescription { AlphaToCoverageEnable = false, IndependentBlendEnable = false };
        var off = new RenderTargetBlendDescription(
            false, false, Blend.One, Blend.Zero, BlendOperation.Add, Blend.One, Blend.Zero, BlendOperation.Add, LogicOp.Noop, ColorWriteEnable.All);

        for (var slot = 0; slot < TargetSlots; slot++)
        {
            blend.RenderTarget[slot] = off;
        }

        return blend;
    }
}
