// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Graphics;
using FiveStar.Graphics.Probes;
using FiveStar.Shaders.Dxil;

namespace FiveStar.Cli.Graphics;

/// <summary>
/// The adapters of <see cref="AdapterCatalog"/> with their mesh-shader wave measured: the probe compiled by the DXIL
/// backend, so <c>plan</c>, <c>pso</c> and setup pick a mesh program's form as the loader does.
/// </summary>
public static class MeasuredAdapters
{
    public static IReadOnlyList<AdapterInfo> List()
    {
        return AdapterCatalog.List(DxilBackend.Compile(MeshWaveProbe.Source(), MeshWaveProbe.Profile));
    }
}
