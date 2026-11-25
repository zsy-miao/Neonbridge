// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Hardware;

/// <summary>
/// Vertex-shader fields of an FSSM header. With a fetch shader the program starts by calling a driver-generated
/// subroutine (PC in <see cref="FetchPcSgpr"/>) that loads the attributes listed in <see cref="FetchInputs"/>.
/// </summary>
public sealed record VertexConfig
{
    /// <summary>VS_VGPR_COMP_CNT: 0 loads only VertexID (v0), 3 also InstanceID (v3).</summary>
    public required int VgprComponentCount { get; init; }

    public required int ParamExports { get; init; }

    public required bool FetchShader { get; init; }

    public int? FetchPcSgpr { get; init; }

    public int? FetchVgprBase { get; init; }

    public required IReadOnlyList<FetchInput> FetchInputs { get; init; }
}
