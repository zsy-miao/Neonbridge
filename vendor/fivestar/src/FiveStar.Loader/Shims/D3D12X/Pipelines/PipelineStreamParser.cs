// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>
/// Reads the pipeline-state stream the game passes to device slot 86 (built by 0x14086f090): each subobject is a u32
/// <see cref="XboxSubobjectType"/>, then its desktop payload at the payload's alignment, every subobject starting
/// pointer-aligned. Shaders are DXBC containers; everything else has the desktop d3d12.h layout.
/// </summary>
internal static unsafe class PipelineStreamParser
{
    private const int TypeSize = sizeof(int);
    private const int WordSize = sizeof(int);

    // D3D12_RT_FORMAT_ARRAY: eight DXGI formats, then NumRenderTargets.
    private const int TargetCountField = PipelineLayout.RenderTargetSlots * WordSize;
    private const int TargetArraySize = TargetCountField + WordSize;

    // DXGI_SAMPLE_DESC {Count, Quality}; D3D12_STREAM_OUTPUT_DESC; D3D12_VIEW_INSTANCING_DESC.
    private const int SampleDescSize = 2 * WordSize;
    private const int StreamOutputSize = 0x20;
    private const int ViewInstancingSize = 0x18;

    // D3D12_INPUT_LAYOUT_DESC {pInputElementDescs, NumElements} and D3D12_INPUT_ELEMENT_DESC fields.
    private const int ElementCountField = PipelineLayout.PointerSize;
    private const int ElementIndexField = 0x08;
    private const int ElementFormatField = 0x0C;
    private const int ElementSlotField = 0x10;
    private const int ElementOffsetField = 0x14;
    private const int ElementClassField = 0x18;
    private const int ElementStepField = 0x1C;

    // More elements than D3D12_IA_VERTEX_INPUT_STRUCTURE_ELEMENT_COUNT means the layout pointer is wrong.
    private const int MaxElements = 32;

    public static GraphicsPipelineDescription Parse(byte* stream, long size)
    {
        var description = GraphicsPipelineDescription.Defaults(0);
        long offset = 0;

        while (offset + TypeSize <= size)
        {
            var type = (XboxSubobjectType)(*(int*)(stream + offset));
            var payload = Align(offset + TypeSize, PayloadAlignment(type));
            var length = PayloadSize(type);

            if (payload + length > size)
            {
                throw new InvalidDataException($"subobject {type} at 0x{offset:x} runs past the stream (0x{size:x} bytes)");
            }

            description = Apply(description, type, stream + payload);
            offset = Align(payload + length, PipelineLayout.PointerSize);
        }

        return description;
    }

    private static GraphicsPipelineDescription Apply(GraphicsPipelineDescription description, XboxSubobjectType type, byte* payload)
    {
        switch (type)
        {
            case XboxSubobjectType.RootSignature:
                return description with { GameRootSignature = *(nint*)payload };
            case XboxSubobjectType.VertexShader or XboxSubobjectType.PixelShader or XboxSubobjectType.DomainShader
                or XboxSubobjectType.HullShader or XboxSubobjectType.GeometryShader or XboxSubobjectType.ComputeShader
                or XboxSubobjectType.AmplificationShader or XboxSubobjectType.MeshShader:
                return Shader(description, payload);
            case XboxSubobjectType.Blend:
                return description with { Blend = GameMemory.Copy((nint)payload, PipelineLayout.BlendDescSize) };
            case XboxSubobjectType.SampleMask:
                return description with { SampleMask = *(uint*)payload };
            case XboxSubobjectType.Rasterizer:
                return description with { Rasterizer = GameMemory.Copy((nint)payload, PipelineLayout.RasterizerDescSize) };
            case XboxSubobjectType.DepthStencil:
                return description with { DepthStencil = DepthStencil(payload) };
            case XboxSubobjectType.DepthStencil1:
                return description with { DepthStencil = GameMemory.Copy((nint)payload, PipelineLayout.DepthStencil1DescSize) };
            case XboxSubobjectType.InputLayout:
                return description with { InputLayout = InputLayout(payload) };
            case XboxSubobjectType.StripCutValue:
                return description with { StripCut = *(int*)payload };
            case XboxSubobjectType.PrimitiveTopology:
                return description with { TopologyType = TopologyTypes.Desktop(*(int*)payload) };
            case XboxSubobjectType.RenderTargetFormats:
                return description with { RenderTargetFormats = Targets(payload) };
            case XboxSubobjectType.DepthStencilFormat:
                return description with { DepthStencilFormat = *(int*)payload };
            case XboxSubobjectType.SampleDescription:
                return description with { SampleCount = *(int*)payload, SampleQuality = *(int*)(payload + WordSize) };
            case XboxSubobjectType.NodeMask or XboxSubobjectType.Flags or XboxSubobjectType.CachedPipelineState:
                return description;
            default:
                throw new NotSupportedException($"pipeline subobject {type} is not translated");
        }
    }

    private static GraphicsPipelineDescription Shader(GraphicsPipelineDescription description, byte* payload)
    {
        var (address, size) = GameMemory.Range(payload, 0);

        if (address == 0 || size == 0)
        {
            return description;
        }

        var source = new ShaderSource(ShaderSourceKind.Container, GameMemory.Copy(address, size));

        return description with { Shaders = [.. description.Shaders, source] };
    }

    private static byte[] DepthStencil(byte* payload)
    {
        return DesktopStateBytes.WidenDepthStencil(new ReadOnlySpan<byte>(payload, PipelineLayout.DepthStencilDescSize));
    }

    private static List<VertexElement> InputLayout(byte* payload)
    {
        var elements = *(byte**)payload;
        var count = *(int*)(payload + ElementCountField);

        if (count < 0 || count > MaxElements || (count > 0 && elements == null))
        {
            throw new InvalidDataException($"input layout of {count} elements at 0x{(nint)elements:x}");
        }

        var layout = new List<VertexElement>(count);

        for (var index = 0; index < count; index++)
        {
            var element = elements + (index * PipelineLayout.InputElementSize);
            layout.Add(new VertexElement(
                GameMemory.Ansi(*(nint*)element),
                *(int*)(element + ElementIndexField),
                *(int*)(element + ElementFormatField),
                *(int*)(element + ElementSlotField),
                *(int*)(element + ElementOffsetField),
                *(int*)(element + ElementClassField),
                *(int*)(element + ElementStepField)));
        }

        return layout;
    }

    private static int[] Targets(byte* payload)
    {
        var count = Math.Clamp(*(int*)(payload + TargetCountField), 0, PipelineLayout.RenderTargetSlots);
        var formats = new int[count];

        for (var slot = 0; slot < count; slot++)
        {
            formats[slot] = *(int*)(payload + (slot * WordSize));
        }

        return formats;
    }

    private static int PayloadAlignment(XboxSubobjectType type)
    {
        switch (type)
        {
            case XboxSubobjectType.RootSignature or XboxSubobjectType.VertexShader or XboxSubobjectType.PixelShader
                or XboxSubobjectType.DomainShader or XboxSubobjectType.HullShader or XboxSubobjectType.GeometryShader
                or XboxSubobjectType.ComputeShader or XboxSubobjectType.AmplificationShader or XboxSubobjectType.MeshShader
                or XboxSubobjectType.StreamOutput or XboxSubobjectType.InputLayout or XboxSubobjectType.CachedPipelineState
                or XboxSubobjectType.ViewInstancing:
                return PipelineLayout.PointerSize;
            default:
                return WordSize;
        }
    }

    private static int PayloadSize(XboxSubobjectType type)
    {
        switch (type)
        {
            case XboxSubobjectType.RootSignature:
                return PipelineLayout.PointerSize;
            case XboxSubobjectType.VertexShader or XboxSubobjectType.PixelShader or XboxSubobjectType.DomainShader
                or XboxSubobjectType.HullShader or XboxSubobjectType.GeometryShader or XboxSubobjectType.ComputeShader
                or XboxSubobjectType.AmplificationShader or XboxSubobjectType.MeshShader
                or XboxSubobjectType.InputLayout or XboxSubobjectType.CachedPipelineState:
                return PipelineLayout.RangeSize;
            case XboxSubobjectType.Blend:
                return PipelineLayout.BlendDescSize;
            case XboxSubobjectType.Rasterizer:
                return PipelineLayout.RasterizerDescSize;
            case XboxSubobjectType.DepthStencil:
                return PipelineLayout.DepthStencilDescSize;
            case XboxSubobjectType.DepthStencil1:
                return PipelineLayout.DepthStencil1DescSize;
            case XboxSubobjectType.RenderTargetFormats:
                return TargetArraySize;
            case XboxSubobjectType.SampleDescription:
                return SampleDescSize;
            case XboxSubobjectType.StreamOutput:
                return StreamOutputSize;
            case XboxSubobjectType.ViewInstancing:
                return ViewInstancingSize;
            case XboxSubobjectType.SampleMask or XboxSubobjectType.StripCutValue or XboxSubobjectType.PrimitiveTopology
                or XboxSubobjectType.DepthStencilFormat or XboxSubobjectType.NodeMask or XboxSubobjectType.Flags:
                return WordSize;
            default:
                throw new InvalidDataException($"unknown pipeline subobject type {(int)type}");
        }
    }

    private static long Align(long value, int alignment)
    {
        return (value + alignment - 1) / alignment * alignment;
    }
}
