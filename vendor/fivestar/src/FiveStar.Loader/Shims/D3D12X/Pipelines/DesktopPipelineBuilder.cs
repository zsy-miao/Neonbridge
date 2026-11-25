// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Hardware;
using Vortice.Direct3D12;

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>
/// Writes the desktop stream of a pipeline: our root signature, the translated programs in the stage their hardware
/// stage asks for (an NGG program is a mesh shader, so such a pipeline has no input assembler) and the game's
/// fixed-function descs, with the combinations only the console takes rewritten (<see cref="DesktopStateRules"/>).
/// </summary>
internal static unsafe class DesktopPipelineBuilder
{
    public static int Graphics(
        nint device2, nint rootSignature, GraphicsPipelineDescription description, IReadOnlyList<TranslatedProgram> programs, Guid* iid, nint* result)
    {
        using var stream = GraphicsStream(device2, rootSignature, description, programs);

        return stream.Create(device2, iid, result);
    }

    /// <summary>The desktop stream of a graphics pipeline; the caller disposes it.</summary>
    public static DesktopStream GraphicsStream(nint device2, nint rootSignature, GraphicsPipelineDescription description, IReadOnlyList<TranslatedProgram> programs)
    {
        var stream = new DesktopStream();
        stream.AddPointer(PipelineStateSubObjectType.RootSignature, rootSignature);
        var mesh = false;

        foreach (var program in programs)
        {
            mesh |= program.Stage == HardwareStage.Ngg;
            stream.AddShader(StageType(program.Stage), program.Dxil);
        }

        stream.AddBytes(PipelineStateSubObjectType.Blend, StatePadding.Blend(DesktopStateRules.Blend(description.Blend)));
        stream.AddWords(PipelineStateSubObjectType.SampleMask, [unchecked((int)description.SampleMask)]);
        stream.AddBytes(PipelineStateSubObjectType.Rasterizer, DesktopStateRules.Rasterizer(description.Rasterizer, description.DepthStencil));
        stream.AddBytes(PipelineStateSubObjectType.DepthStencil1, StatePadding.DepthStencil(description.DepthStencil));
        DebugLayer.DepthUavs.NoteDepthState(programs, description.DepthStencil);

        if (mesh)
        {
            // The runtime keeps a mesh pipeline's topology type as its shader's output topology and refuses a library
            // load whose stream leaves it out (debug-layer warning 972); translated mesh programs output triangles.
            stream.AddWords(PipelineStateSubObjectType.PrimitiveTopology, [PipelineLayout.TriangleTopology]);
        }
        else
        {
            InputAssembler(stream, description);
        }

        stream.AddWords(PipelineStateSubObjectType.RenderTargetFormats, TargetWords(device2, description.RenderTargetFormats));
        stream.AddWords(PipelineStateSubObjectType.DepthStencilFormat, [description.DepthStencilFormat]);
        stream.AddWords(PipelineStateSubObjectType.SampleDescription, [description.SampleCount, description.SampleQuality]);

        return stream;
    }

    /// <summary>The desktop stream of a compute pipeline; the caller disposes it.</summary>
    public static DesktopStream ComputeStream(nint rootSignature, TranslatedProgram program)
    {
        var stream = new DesktopStream();
        stream.AddPointer(PipelineStateSubObjectType.RootSignature, rootSignature);
        stream.AddShader(PipelineStateSubObjectType.ComputeShader, program.Dxil);

        return stream;
    }

    private static void InputAssembler(DesktopStream stream, GraphicsPipelineDescription description)
    {
        if (description.InputLayout.Count > 0)
        {
            stream.AddInputLayout(description.InputLayout);
        }

        if (description.StripCut != 0)
        {
            stream.AddWords(PipelineStateSubObjectType.IndexBufferStripCutValue, [description.StripCut]);
        }

        stream.AddWords(PipelineStateSubObjectType.PrimitiveTopology, [description.TopologyType]);
    }

    /// <summary>D3D12_RT_FORMAT_ARRAY: eight DXGI formats (render-target substitutes in, <see cref="TargetFormats"/>), then the target count.</summary>
    private static int[] TargetWords(nint device, IReadOnlyList<int> formats)
    {
        var words = new int[PipelineLayout.RenderTargetSlots + 1];

        for (var slot = 0; slot < formats.Count && slot < PipelineLayout.RenderTargetSlots; slot++)
        {
            words[slot] = TargetFormats.GpuWritten(device, formats[slot]);
        }

        words[PipelineLayout.RenderTargetSlots] = Math.Min(formats.Count, PipelineLayout.RenderTargetSlots);

        return words;
    }

    private static PipelineStateSubObjectType StageType(HardwareStage stage)
    {
        switch (stage)
        {
            case HardwareStage.Vertex:
                return PipelineStateSubObjectType.VertexShader;
            case HardwareStage.Pixel:
                return PipelineStateSubObjectType.PixelShader;
            case HardwareStage.Geometry:
                return PipelineStateSubObjectType.GeometryShader;
            case HardwareStage.Ngg:
                return PipelineStateSubObjectType.MeshShader;
            default:
                throw new NotSupportedException($"a {stage} program in a graphics pipeline");
        }
    }
}
