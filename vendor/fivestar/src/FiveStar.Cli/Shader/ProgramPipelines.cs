// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Graphics.Pipelines;
using FiveStar.Shaders.Compilation;
using FiveStar.Shaders.Containers;
using Vortice.Direct3D12;
using Vortice.DXGI;

namespace FiveStar.Cli.Shader;

/// <summary>
/// Builds the pipeline state of one compiled program: CS, its wave32 variant and the mesh fallback CS as compute
/// pipelines, VS without a pixel shader, PS and GS behind a generated pass-through VS, MS without a pixel shader; or of
/// a linked producer with its pixel shader (vs+ps, msfb_vs+ps, ms+ps).
/// </summary>
internal sealed class ProgramPipelines
{
    // D3D_PRIMITIVE input of a GS (PSV0 stage info): 1 point, 2 line, 6 line with adjacency.
    private const int PointPrimitive = 1;
    private const int LinePrimitive = 2;
    private const int LineAdjacencyPrimitive = 6;

    private readonly PipelineBuilder _builder;
    private readonly PassThroughCache _passThrough;

    public ProgramPipelines(PipelineBuilder builder, PassThroughCache passThrough)
    {
        _builder = builder;
        _passThrough = passThrough;
    }

    /// <summary>
    /// Describes the pipeline (a <see cref="PipelineSetupException"/> when that fails) and creates it (the driver's
    /// exception when it refuses).
    /// </summary>
    public ID3D12PipelineState Create(PsoTarget target, byte[] program)
    {
        switch (target.Kind)
        {
            case "cs":
            case "cs_w32":
            case "msfb_cs":
                return _builder.Compute(program);
            case "vs":
            case "msfb_vs":
                return Vertex(program);
            case "ps":
                return Pixel(program);
            case "gs":
                return Geometry(program);
            case "ms":
            case "ms_w32":
                return _builder.Mesh(new MeshProgram(program, null, [], Format.Unknown));
            case "vs+ps":
            case "msfb_vs+ps":
                return VertexPixel(program, Consumer(target));
            case "ms+ps":
                return MeshPixel(program, Consumer(target));
            default:
                throw new PipelineSetupException($"unknown kind {target.Kind}");
        }
    }

    private ID3D12PipelineState Vertex(byte[] program)
    {
        var layout = SignatureFormats.InputLayout(InputSignature(program));

        return _builder.Graphics(new GraphicsProgram(program, null, null, layout, [], Format.Unknown, PrimitiveTopologyType.Triangle));
    }

    private static byte[] Consumer(PsoTarget target)
    {
        if (target.ConsumerPath is null || !File.Exists(target.ConsumerPath))
        {
            throw new PipelineSetupException("no pixel shader DXIL");
        }

        return File.ReadAllBytes(target.ConsumerPath);
    }

    /// <summary>A linked VS + PS: the input layout from the VS, the targets from the PS outputs.</summary>
    private ID3D12PipelineState VertexPixel(byte[] vertex, byte[] pixel)
    {
        var outputs = PsvSignatureParser.Parse(ProgramChunks.Psv(pixel).Span).Outputs;
        var layout = SignatureFormats.InputLayout(InputSignature(vertex));
        var program = new GraphicsProgram(vertex, null, pixel, layout, SignatureFormats.Targets(outputs), SignatureFormats.Depth(outputs), PrimitiveTopologyType.Triangle);

        return _builder.Graphics(program);
    }

    private ID3D12PipelineState MeshPixel(byte[] mesh, byte[] pixel)
    {
        var outputs = PsvSignatureParser.Parse(ProgramChunks.Psv(pixel).Span).Outputs;

        return _builder.Mesh(new MeshProgram(mesh, pixel, SignatureFormats.Targets(outputs), SignatureFormats.Depth(outputs)));
    }

    private static IReadOnlyList<SignatureElement> InputSignature(byte[] program)
    {
        var inputs = DxbcContainer.Parse(program).Find("ISG1");

        if (inputs is null)
        {
            return [];
        }

        return SignatureParser.Parse(inputs.Data.Span);
    }

    private ID3D12PipelineState Pixel(byte[] program)
    {
        var psv = PsvSignatureParser.Parse(ProgramChunks.Psv(program).Span);
        var vertex = PassThrough(psv.Inputs, needsPosition: true);
        var targets = SignatureFormats.Targets(psv.Outputs);
        var depth = SignatureFormats.Depth(psv.Outputs);

        return _builder.Graphics(new GraphicsProgram(vertex, null, program, [], targets, depth, PrimitiveTopologyType.Triangle));
    }

    private ID3D12PipelineState Geometry(byte[] program)
    {
        var chunk = ProgramChunks.Psv(program).Span;
        var vertex = PassThrough(PsvSignatureParser.Parse(chunk).Inputs, needsPosition: false);
        var primitive = PipelineStateParser.Parse(chunk).GeometryInputPrimitive;
        var topology = PrimitiveTopologyType.Triangle;

        if (primitive == PointPrimitive)
        {
            topology = PrimitiveTopologyType.Point;
        }
        else if (primitive is LinePrimitive or LineAdjacencyPrimitive)
        {
            topology = PrimitiveTopologyType.Line;
        }

        return _builder.Graphics(new GraphicsProgram(vertex, program, null, [], [], Format.Unknown, topology));
    }

    private byte[] PassThrough(IReadOnlyList<PsvElement> inputs, bool needsPosition)
    {
        try
        {
            return _passThrough.Compile(PassThroughVertexShader.Source(inputs, needsPosition));
        }
        catch (Exception exception) when (exception is InvalidOperationException or NotSupportedException)
        {
            throw new PipelineSetupException(exception.Message);
        }
    }
}
