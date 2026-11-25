// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Graphics;

/// <summary>
/// A DXGI adapter and its D3D12 capabilities. <see cref="ShaderModel"/> is null when no D3D12 device could be
/// created on it; the wave lane counts are D3D12_FEATURE_DATA_D3D12_OPTIONS1.WaveLaneCountMin/Max.
/// <see cref="MeshWaveLanes"/> is the narrowest wave the driver ran a probe mesh shader in (<see cref="Probes.MeshWaveProbe"/>),
/// null when not measured: then a mesh shader may run anywhere in Min-Max.
/// </summary>
public sealed record AdapterInfo(
    int Index,
    string Name,
    bool Software,
    string? ShaderModel,
    int WaveLaneCountMin,
    int WaveLaneCountMax,
    string? MeshShaderTier)
{
    private const string NoMeshShaders = "NotSupported";

    /// <summary>The narrowest wave a probe mesh shader ran in on this adapter; null when not measured.</summary>
    public int? MeshWaveLanes { get; init; }

    /// <summary>Whether the adapter runs mesh shaders (D3D12_OPTIONS7 mesh-shader tier 1 or above).</summary>
    public bool MeshShaders => MeshShaderTier is not null && MeshShaderTier != NoMeshShaders;
}
