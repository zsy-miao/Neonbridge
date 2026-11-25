// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Graphics;
using FiveStar.Graphics.Probes;
using FiveStar.Loader.Shims.D3D12X.Pipelines;
using FiveStar.Loader.Tests.Support;
using FiveStar.Shaders.Dxil;
using Xunit;

namespace FiveStar.Loader.Tests;

/// <summary>
/// The mesh-wave probe runs on WARP and reports a wave inside the range the adapter states, and the desktop adapter's
/// description carries it, so a mesh program's form follows the wave the driver runs mesh shaders in.
/// </summary>
public sealed class MeshWaveProbeTests
{
    [WarpFact]
    public void The_probe_measures_the_wave_mesh_shaders_run_in_and_the_adapter_carries_it()
    {
        using var device = GpuDevice.CreateWarp(debugLayer: false);
        var adapter = DesktopAdapter.Describe(device.Device.NativePointer);

        if (!adapter.MeshShaders)
        {
            return;
        }

        var lanes = MeshWaveProbe.Measure(device.Device, DxilBackend.Compile(MeshWaveProbe.Source(), MeshWaveProbe.Profile));

        Assert.NotNull(lanes);
        Assert.InRange(lanes.Value, adapter.WaveLaneCountMin, adapter.WaveLaneCountMax);
        Assert.Equal(lanes, adapter.MeshWaveLanes);
    }
}
