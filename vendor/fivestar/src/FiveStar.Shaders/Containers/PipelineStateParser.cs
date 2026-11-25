// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;
using System.Globalization;

namespace FiveStar.Shaders.Containers;

/// <summary>Reads a PSV0 chunk: u32 info size, then PSVRuntimeInfo0/1/2 (DirectXShaderCompiler DxilPipelineStateValidation.h).</summary>
public static class PipelineStateParser
{
    private const int InfoStart = 4;

    // PSVRuntimeInfo0: 16-byte stage union @0, u32 min/max wave lanes @16/@20.
    private const int MinimumWaveField = 16;
    private const int MaximumWaveField = 20;

    // PSVRuntimeInfo1 (info size >= 36): u8 shader stage @24, u8 uses view id @25,
    // then the stage-specific union at @26, then u8 signature input / output /
    // patch-constant-or-primitive element counts @28..30. A mesh program's
    // union is {u8 SigPrimVectors @26, u8 MeshOutputTopology @27}.
    private const int Info1Size = 36;
    private const int StageField = 24;
    private const int ViewIdField = 25;
    private const int StageSpecificField = 26;
    private const int MeshTopologyField = 27;
    private const int SignatureInputField = 28;
    private const int SignatureOutputField = 29;
    private const int SignaturePrimitiveField = 30;

    // PSVRuntimeInfo2 (info size >= 48): u32 numthreads x/y/z @36/@40/@44.
    private const int Info2Size = 48;
    private const int NumThreadsXField = 36;
    private const int NumThreadsYField = 40;
    private const int NumThreadsZField = 44;

    // Stage union: mesh {u32 groupshared bytes, u32 view-id groupshared bytes, u32 payload bytes, u16 max vertices,
    // u16 max primitives}; geometry {u32 input primitive, u32 output topology, u32 stream mask}; compute {u32 groupshared bytes}.
    private const int PayloadField = 8;
    private const int MaxVerticesField = 12;
    private const int MaxPrimitivesField = 14;
    private const int GeometryTopologyField = 4;
    private const int GeometryStreamMaskField = 8;

    // PSVShaderKind numbering.
    private static readonly string[] Kinds = ["ps", "vs", "gs", "hs", "ds", "cs", "lib", "rg", "is", "ah", "ch", "ms", "cl", "ms", "as"];

    // Mesh output topology (DXIL::MeshOutputTopology).
    private static readonly IReadOnlyDictionary<int, string> Topologies = new Dictionary<int, string>
    {
        [1] = "line",
        [2] = "triangle",
    };

    /// <summary>The runtime info of a PSV0 chunk; <see cref="InvalidDataException"/> when its sizes do not fit the chunk.</summary>
    public static PipelineStateInfo Parse(ReadOnlySpan<byte> chunk)
    {
        if (chunk.Length < InfoStart)
        {
            throw new InvalidDataException("PSV0 chunk shorter than its info size");
        }

        var size = BinaryPrimitives.ReadInt32LittleEndian(chunk);

        if (size < MaximumWaveField + sizeof(int) || size > chunk.Length - InfoStart)
        {
            throw new InvalidDataException($"PSV0 info size {size} does not fit a chunk of {chunk.Length} bytes");
        }

        var info = chunk.Slice(InfoStart, size);
        var result = new PipelineStateInfo
        {
            InfoSize = size,
            MinimumWaveLanes = ReadInt(info, MinimumWaveField),
            MaximumWaveLanes = ReadInt(info, MaximumWaveField),
        };

        if (size >= Info1Size)
        {
            result = WithInfo1(result, info);
        }

        if (size >= Info2Size)
        {
            var threads = new ThreadGroupSize(
                ReadInt(info, NumThreadsXField),
                ReadInt(info, NumThreadsYField),
                ReadInt(info, NumThreadsZField));
            result = result with { NumThreads = threads };
        }

        return result;
    }

    private static PipelineStateInfo WithInfo1(PipelineStateInfo result, ReadOnlySpan<byte> info)
    {
        var kind = info[StageField];
        var stage = kind.ToString(CultureInfo.InvariantCulture);

        if (kind < Kinds.Length)
        {
            stage = Kinds[kind];
        }

        result = result with
        {
            Stage = stage,
            UsesViewId = info[ViewIdField],
            SignatureInputCount = info[SignatureInputField],
            SignatureOutputCount = info[SignatureOutputField],
            SignaturePatchConstantOrPrimitiveCount = info[SignaturePrimitiveField],
        };

        switch (stage)
        {
            case "ms":
                return WithMesh(result, info);
            case "gs":
                return WithGeometry(result, info);
            case "cs":
                return result with { LdsBytes = ReadInt(info, 0) };
            default:
                return result;
        }
    }

    private static PipelineStateInfo WithMesh(PipelineStateInfo result, ReadOnlySpan<byte> info)
    {
        var topologyCode = info[MeshTopologyField];
        var topology = topologyCode.ToString(CultureInfo.InvariantCulture);

        if (Topologies.TryGetValue(topologyCode, out var name))
        {
            topology = name;
        }

        return result with
        {
            LdsBytes = ReadInt(info, 0),
            PayloadBytes = ReadInt(info, PayloadField),
            MaxVertices = BinaryPrimitives.ReadUInt16LittleEndian(info[MaxVerticesField..]),
            MaxPrimitives = BinaryPrimitives.ReadUInt16LittleEndian(info[MaxPrimitivesField..]),
            Topology = topology,
        };
    }

    private static PipelineStateInfo WithGeometry(PipelineStateInfo result, ReadOnlySpan<byte> info)
    {
        return result with
        {
            GeometryInputPrimitive = ReadInt(info, 0),
            GeometryOutputTopology = ReadInt(info, GeometryTopologyField),
            GeometryStreamMask = ReadInt(info, GeometryStreamMaskField),
            MaxVertexCount = BinaryPrimitives.ReadUInt16LittleEndian(info[StageSpecificField..]),
        };
    }

    private static int ReadInt(ReadOnlySpan<byte> info, int at)
    {
        return BinaryPrimitives.ReadInt32LittleEndian(info[at..]);
    }
}
