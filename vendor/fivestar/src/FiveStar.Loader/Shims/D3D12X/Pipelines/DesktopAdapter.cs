// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Graphics;
using FiveStar.Graphics.Probes;
using FiveStar.Loader.Native;
using FiveStar.Shaders.Dxil;
using Vortice.Direct3D12;

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>
/// The capabilities of the desktop device that decide which translated form of a program it runs: shader model,
/// D3D12_OPTIONS1 wave lane counts, the D3D12_OPTIONS7 mesh-shader tier and, with mesh shaders, the wave the driver runs
/// them in (<see cref="MeshWaveProbe"/>, measured once per adapter: a device's address is reused after it is freed,
/// so a cache by device handed a new device the measurement of an earlier one, or its failure).
/// </summary>
internal static unsafe class DesktopAdapter
{
    // D3D12_FEATURE values (d3d12.h).
    private const int ShaderModelFeature = 7;
    private const int Options1Feature = 8;
    private const int Options7Feature = 32;

    // D3D_SHADER_MODEL: major in the high nibble, minor in the low one; asked from the newest the runtime may know.
    private const int NewestShaderModel = 0x69;
    private const int OldestShaderModel = 0x60;
    private const int MajorShift = 4;
    private const int MinorMask = 0xF;

    // D3D12_FEATURE_DATA_D3D12_OPTIONS1: WaveOps, WaveLaneCountMin, WaveLaneCountMax, TotalLaneCount,
    // ExpandedComputeResourceStates, Int64ShaderOps.
    private const int Options1Words = 6;
    private const int WaveLaneCountMinWord = 1;
    private const int WaveLaneCountMaxWord = 2;

    // D3D12_FEATURE_DATA_D3D12_OPTIONS7: MeshShaderTier (D3D12_MESH_SHADER_TIER_1 = 10), SamplerFeedbackTier.
    private const int Options7Words = 2;
    private const int MeshShaderTierWord = 0;
    private const int MeshTierScale = 10;

    private const string AdapterName = "desktop device";
    private const string NoMeshShaders = "NotSupported";
    private const string MeshTierPrefix = "Tier";

    private static readonly Dictionary<long, int> MeshWaves = [];

    public static AdapterInfo Describe(nint device)
    {
        var options1 = stackalloc int[Options1Words];
        var options7 = stackalloc int[Options7Words];
        var meshTier = NoMeshShaders;
        var waveMin = 0;
        var waveMax = 0;

        if (!HResult.Failed(DesktopDevice.CheckFeatureSupport(device, Options1Feature, options1, Options1Words * sizeof(int))))
        {
            waveMin = options1[WaveLaneCountMinWord];
            waveMax = options1[WaveLaneCountMaxWord];
        }

        if (!HResult.Failed(DesktopDevice.CheckFeatureSupport(device, Options7Feature, options7, Options7Words * sizeof(int)))
            && options7[MeshShaderTierWord] != 0)
        {
            meshTier = MeshTierPrefix + (options7[MeshShaderTierWord] / MeshTierScale);
        }

        int? meshWave = null;

        if (meshTier != NoMeshShaders)
        {
            meshWave = MeshWave(device);
        }

        return new AdapterInfo(0, AdapterName, false, ShaderModel(device), waveMin, waveMax, meshTier) { MeshWaveLanes = meshWave };
    }

    /// <summary>The measured mesh-shader wave of <paramref name="device"/>'s adapter, probed until a probe succeeds.</summary>
    private static int? MeshWave(nint device)
    {
        Marshal.AddRef(device);
        using var desktop = new ID3D12Device(device);
        long adapter = desktop.AdapterLuid;

        lock (MeshWaves)
        {
            if (MeshWaves.TryGetValue(adapter, out int known))
            {
                return known;
            }

            int? lanes = MeshWaveProbe.Measure(desktop, DxilBackend.Compile(MeshWaveProbe.Source(), MeshWaveProbe.Profile));
            ShimContext.Log?.Info($"  [d3d12_xs] mesh shaders run in {Lanes(lanes)} waves (probe)");

            if (lanes is int measured)
            {
                MeshWaves[adapter] = measured;
            }

            return lanes;
        }
    }

    private static string Lanes(int? lanes)
    {
        if (lanes is null)
        {
            return "unmeasured";
        }

        return $"{lanes}-lane";
    }

    private static string? ShaderModel(nint device)
    {
        for (var model = NewestShaderModel; model >= OldestShaderModel; model--)
        {
            var data = model;

            if (!HResult.Failed(DesktopDevice.CheckFeatureSupport(device, ShaderModelFeature, &data, sizeof(int))))
            {
                return $"{data >> MajorShift}.{data & MinorMask}";
            }
        }

        return null;
    }
}
