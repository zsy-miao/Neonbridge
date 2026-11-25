// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Dxil.Container;

/// <summary>
/// What the container and the entry metadata say about a shader: thread-group size, required wave size, shader flags
/// and the size of the user-data constant buffer at b0 (0: none); for graphics stages the kind, the signatures and the
/// stage's own state.
/// </summary>
public sealed record ShaderDescription((int X, int Y, int Z) Threads, int? WaveSize, ShaderFlagBits Flags, int UserDataBytes)
{
    public DxilShaderKind Kind { get; init; } = DxilShaderKind.Compute;

    public IReadOnlyList<DxilSignatureElement> Inputs { get; init; } = [];

    public IReadOnlyList<DxilSignatureElement> Outputs { get; init; } = [];

    /// <summary>A mesh shader's per-primitive outputs.</summary>
    public IReadOnlyList<DxilSignatureElement> Primitives { get; init; } = [];

    /// <summary>A pixel shader's wave operations count helper lanes (<c>[WaveOpsIncludeHelperLanes]</c>).</summary>
    public bool IncludeHelperLanes { get; init; }

    public MeshState? Mesh { get; init; }

    public GeometryState? Geometry { get; init; }

    public bool IsGraphics => Kind != DxilShaderKind.Compute;
}
