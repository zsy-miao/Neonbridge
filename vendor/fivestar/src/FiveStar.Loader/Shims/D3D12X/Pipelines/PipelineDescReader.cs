// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Cache;

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>
/// Reads the descriptions of device slots 55, 57, 62 and 67 (docs/d3d12x-pipelines.md). The pipeline-cache ones point
/// into the game's copy of the cache from shaders.rpf: state and input-layout metadata, and code blocks.
/// </summary>
internal static unsafe class PipelineDescReader
{
    /// <summary>Slot 62: D3D12_COMPUTE_PIPELINE_STATE_DESC; the CS bytecode is a DXBC container.</summary>
    public static ComputePipelineDescription Compute(byte* desc)
    {
        var (address, size) = GameMemory.Range(desc, PipelineLayout.ComputeShaderField);

        return new ComputePipelineDescription(*(nint*)desc, Shader(ShaderSourceKind.Container, address, size));
    }

    /// <summary>Slot 67: the pipeline-cache compute description.</summary>
    public static ComputePipelineDescription CachedCompute(byte* desc)
    {
        var (address, size) = GameMemory.Range(desc, PipelineLayout.CachedComputeShaderField);

        return new ComputePipelineDescription(*(nint*)desc, Shader(ShaderSourceKind.CacheCode, address, size));
    }

    /// <summary>Slot 57: the pipeline-cache graphics description, its state decoded from the context registers.</summary>
    public static GraphicsPipelineDescription CachedGraphics(byte* desc)
    {
        var (stateAddress, stateSize) = GameMemory.Range(desc, PipelineLayout.CachedStateField);
        var (layoutAddress, layoutSize) = GameMemory.Range(desc, PipelineLayout.CachedInputLayoutField);
        var shaders = new List<ShaderSource>();

        for (var slot = 0; slot < PipelineLayout.CachedSlotCount; slot++)
        {
            var (code, codeSize) = GameMemory.Range(desc, PipelineLayout.CachedSlotsField + (slot * PipelineLayout.CachedSlotSize));

            if (code != 0 && codeSize > 0)
            {
                shaders.Add(Shader(ShaderSourceKind.CacheCode, code, codeSize));
            }
        }

        return CachedGraphics(GameMemory.Copy(stateAddress, stateSize), GameMemory.Copy(layoutAddress, layoutSize), shaders, *(nint*)desc);
    }

    /// <summary>
    /// A pipeline-cache graphics record from its state metadata, its 0x0C metadata (formats and input layout) and the
    /// code blocks of its slots in slot order: what slot 57 reads from game memory, and the warm-up from the cache blob.
    /// </summary>
    public static GraphicsPipelineDescription CachedGraphics(byte[] stateMeta, byte[] layoutMeta, IReadOnlyList<ShaderSource> shaders, nint gameRootSignature)
    {
        var registers = GraphicsStateParser.Parse(stateMeta);

        if (registers is null)
        {
            throw new NotSupportedException($"pipeline-cache state of 0x{stateMeta.Length:x} bytes has an unknown layout");
        }

        var state = FixedFunctionDecoder.Decode(registers);

        return CachedStateMapping.Describe(state, Targets(layoutMeta), gameRootSignature, shaders, Elements(layoutMeta));
    }

    /// <summary>The formats the record's 0x0C entry stores ahead of the input layout; null when the entry is absent.</summary>
    private static PipelineTargets? Targets(byte[] meta)
    {
        if (meta.Length == 0)
        {
            return null;
        }

        return PipelineTargetsParser.Parse(meta);
    }

    /// <summary>Slot 55: <paramref name="count"/> override entries of <see cref="PipelineLayout.OverrideStride"/> bytes.</summary>
    public static IReadOnlyList<PipelineStateOverride> Overrides(uint count, byte* entries)
    {
        if (count > 0 && entries == null)
        {
            throw new InvalidDataException($"{count} overrides without an array");
        }

        var overrides = new List<PipelineStateOverride>((int)count);

        for (var index = 0; index < count; index++)
        {
            var entry = entries + (index * PipelineLayout.OverrideStride);
            var payload = GameMemory.Copy((nint)(entry + PipelineLayout.OverridePayloadField), PipelineLayout.OverrideStride - PipelineLayout.OverridePayloadField);
            overrides.Add(new PipelineStateOverride(*(int*)entry, payload));
        }

        return overrides;
    }

    /// <summary>A pipeline-cache code block as slots 57 and 67 keep it: its first <see cref="ProgramIndex.PrefixSize"/> bytes.</summary>
    public static ShaderSource CacheCode(ReadOnlySpan<byte> code)
    {
        return new ShaderSource(ShaderSourceKind.CacheCode, code[..Math.Min(code.Length, ProgramIndex.PrefixSize)].ToArray());
    }

    private static ShaderSource Shader(ShaderSourceKind kind, nint address, long size)
    {
        if (address == 0 || size <= 0)
        {
            throw new InvalidDataException("pipeline description without its shader");
        }

        if (kind == ShaderSourceKind.CacheCode)
        {
            return new ShaderSource(kind, GameMemory.CopyPrefix(address, size, ProgramIndex.PrefixSize));
        }

        return new ShaderSource(kind, GameMemory.Copy(address, size));
    }

    private static List<VertexElement> Elements(byte[] meta)
    {
        if (meta.Length == 0)
        {
            return [];
        }

        return InputLayoutParser.Parse(meta)
            .Select(element => new VertexElement(
                element.Semantic, element.SemanticIndex, element.Format, element.InputSlot, element.Offset, element.Classification, element.StepRate))
            .ToList();
    }
}
