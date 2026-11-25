// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;

namespace FiveStar.Shaders.Cache;

/// <summary>
/// Reads the head of a graphics pipeline's input-layout metadata: u8 RTVFormats[8] at 0x10, u32 SampleMask at 0x1C,
/// u8 DSVFormat, NumRenderTargets and PrimitiveTopologyType (low 4 bits) at 0x24..0x26, u8 sample count at 0x28.
/// </summary>
public static class PipelineTargetsParser
{
    private const int FormatsField = 0x10;
    private const int SampleMaskField = 0x1C;
    private const int DepthStencilField = 0x24;
    private const int CountField = 0x25;
    private const int TopologyField = 0x26;
    private const int SampleCountField = 0x28;
    private const int TopologyMask = 0xF;

    /// <summary>D3D12's limit on simultaneous render targets (the size of RTVFormats).</summary>
    private const int MaxTargets = 8;

    /// <summary>The pipeline's output formats, topology type and sampling.</summary>
    public static PipelineTargets Parse(ReadOnlySpan<byte> meta)
    {
        if (meta.Length <= SampleCountField)
        {
            throw new InvalidDataException("input layout metadata is shorter than its header");
        }

        int count = meta[CountField];

        if (count > MaxTargets)
        {
            throw new InvalidDataException($"render-target count {count} is out of range");
        }

        var formats = new int[count];

        for (var target = 0; target < count; target++)
        {
            formats[target] = meta[FormatsField + target];
        }

        return new PipelineTargets(
            formats,
            meta[DepthStencilField],
            meta[TopologyField] & TopologyMask,
            meta[SampleCountField],
            BinaryPrimitives.ReadUInt32LittleEndian(meta[SampleMaskField..]));
    }
}
