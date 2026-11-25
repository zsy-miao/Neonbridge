// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Containers;

/// <summary>
/// Runtime information from a PSV0 chunk. Stage-specific members are null for other stages, and members of
/// the newer info revisions are null when the chunk is too short to hold them.
/// </summary>
public sealed record PipelineStateInfo
{
    public int InfoSize { get; init; }

    public int MinimumWaveLanes { get; init; }

    public int MaximumWaveLanes { get; init; }

    /// <summary>Shader kind (ps, vs, gs, cs, ms, ...), or the raw number for an unknown kind.</summary>
    public string? Stage { get; init; }

    public int? UsesViewId { get; init; }

    /// <summary>Mesh: groupshared bytes; compute: groupshared bytes.</summary>
    public int? LdsBytes { get; init; }

    public int? PayloadBytes { get; init; }

    public int? MaxVertices { get; init; }

    public int? MaxPrimitives { get; init; }

    /// <summary>Mesh output topology (line, triangle or the raw number).</summary>
    public string? Topology { get; init; }

    /// <summary>Geometry input primitive (D3D_PRIMITIVE numbering).</summary>
    public int? GeometryInputPrimitive { get; init; }

    /// <summary>Geometry output topology (D3D_PRIMITIVE_TOPOLOGY numbering).</summary>
    public int? GeometryOutputTopology { get; init; }

    public int? GeometryStreamMask { get; init; }

    public int? MaxVertexCount { get; init; }

    public int? SignatureInputCount { get; init; }

    public int? SignatureOutputCount { get; init; }

    public int? SignaturePatchConstantOrPrimitiveCount { get; init; }

    /// <summary>numthreads (x, y, z) of compute and mesh programs.</summary>
    public ThreadGroupSize? NumThreads { get; init; }
}
