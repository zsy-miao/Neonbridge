// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Graphics.Probes;
using Vortice.Direct3D12;
using Vortice.DXGI;
using Feature = Vortice.Direct3D12.Feature;

namespace FiveStar.Graphics;

/// <summary>Lists the DXGI adapters with the D3D12 capabilities that decide whether translated programs load.</summary>
public static class AdapterCatalog
{
    private const int MajorShift = 4;
    private const int MinorMask = 0xF;

    /// <summary>Every adapter in DXGI order (the index <see cref="GpuDevice"/> takes), including software ones.</summary>
    public static IReadOnlyList<AdapterInfo> List()
    {
        return List(null);
    }

    /// <summary>The same, with the mesh-shader wave measured by <paramref name="meshProbe"/> (<see cref="MeshWaveProbe"/>'s DXIL) when given.</summary>
    public static IReadOnlyList<AdapterInfo> List(byte[]? meshProbe)
    {
        var adapters = new List<AdapterInfo>();
        using var factory = DXGI.CreateDXGIFactory2<IDXGIFactory4>(false);

        for (uint index = 0; factory.EnumAdapters1(index, out var adapter).Success; index++)
        {
            using (adapter)
            {
                var description = adapter.Description1;
                var software = (description.Flags & AdapterFlags.Software) != 0;
                adapters.Add(Describe((int)index, description.Description, software, meshProbe));
            }
        }

        return adapters;
    }

    private static AdapterInfo Describe(int index, string name, bool software, byte[]? meshProbe)
    {
        try
        {
            using var device = new GpuDevice(index, debugLayer: false);
            var options1 = default(FeatureDataD3D12Options1);
            var options7 = default(FeatureDataD3D12Options7);
            Check(device.Device, Feature.Options1, ref options1);
            Check(device.Device, Feature.Options7, ref options7);
            int? meshWave = null;

            if (meshProbe is not null && options7.MeshShaderTier != MeshShaderTier.NotSupported)
            {
                meshWave = MeshWaveProbe.Measure(device.Device, meshProbe);
            }

            return new AdapterInfo(
                index,
                name,
                software,
                ShaderModelText(HighestShaderModel(device.Device)),
                (int)options1.WaveLaneCountMin,
                (int)options1.WaveLaneCountMax,
                options7.MeshShaderTier.ToString())
            {
                MeshWaveLanes = meshWave,
            };
        }
        catch (SharpGen.Runtime.SharpGenException)
        {
            return new AdapterInfo(index, name, software, null, 0, 0, null);
        }
    }

    /// <summary>The highest shader model the driver reports, asking from the newest model the headers know downward.</summary>
    private static ShaderModel HighestShaderModel(ID3D12Device device)
    {
        var models = Enum.GetValues<ShaderModel>().Where(model => model != ShaderModel.ShaderModelNone).Distinct().OrderDescending();

        foreach (var model in models)
        {
            var data = new FeatureDataShaderModel { HighestShaderModel = model };

            if (TryCheck(device, Feature.ShaderModel, ref data))
            {
                return data.HighestShaderModel;
            }
        }

        return ShaderModel.ShaderModelNone;
    }

    /// <summary>D3D_SHADER_MODEL packs the major version in the high nibble and the minor in the low one.</summary>
    private static string ShaderModelText(ShaderModel model)
    {
        var value = (int)model;

        return $"{value >> MajorShift}.{value & MinorMask}";
    }

    private static void Check<T>(ID3D12Device device, Feature feature, ref T data)
        where T : unmanaged
    {
        if (!TryCheck(device, feature, ref data))
        {
            throw new InvalidOperationException($"CheckFeatureSupport({feature}) failed");
        }
    }

    private static bool TryCheck<T>(ID3D12Device device, Feature feature, ref T data)
        where T : unmanaged
    {
        return device.CheckFeatureSupport(feature, ref data);
    }
}
