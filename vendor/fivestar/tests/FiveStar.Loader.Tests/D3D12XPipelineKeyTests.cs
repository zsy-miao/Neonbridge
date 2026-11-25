// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;
using System.Diagnostics;
using FiveStar.Graphics;
using FiveStar.Loader.Shims.D3D12X.Pipelines;
using FiveStar.Loader.Tests.Support;
using FiveStar.Shaders.Hardware;
using Xunit;

namespace FiveStar.Loader.Tests;

/// <summary>
/// The pipeline cache key on D3D12 WARP: a pipeline stored under its key loads back with the stream it was built
/// from (a mesh pipeline too, which the runtime refused while the stream left its topology out), a NaN depth bias
/// builds, and the key changes with the render-target formats.
/// </summary>
public sealed unsafe class D3D12XPipelineKeyTests
{
    private const string Vertex = "float4 main(float4 p : POSITION) : SV_Position { return p; }";
    private const string Pixel = "float4 main() : SV_Target { return 1; }";
    private const string Mesh = """
        struct V { float4 p : SV_Position; };
        [outputtopology("triangle")] [numthreads(1, 1, 1)]
        void main(out vertices V v[3], out indices uint3 t[1]) { SetMeshOutputCounts(3, 1); v[0].p = 0; v[1].p = 0; v[2].p = 0; t[0] = uint3(0, 1, 2); }
        """;

    private const int Rgba32Float = 2;
    private const int Rgba8Unorm = 28;
    private const int B5G6R5Unorm = 85;

    [DxcFact]
    public void Stored_pipelines_load_back_under_their_keys()
    {
        var folder = Directory.CreateTempSubdirectory("fivestar-psokey-");

        try
        {
            using var device = GpuDevice.CreateWarp(debugLayer: false);
            var (device2, root) = DesktopObjects(device.Device.NativePointer);
            var vertex = new TranslatedProgram("vs", HardwareStage.Vertex, Compile(folder.FullName, "vs_6_6", Vertex));
            var pixel = new TranslatedProgram("ps", HardwareStage.Pixel, Compile(folder.FullName, "ps_6_6", Pixel));
            var mesh = new TranslatedProgram("ms", HardwareStage.Ngg, Compile(folder.FullName, "ms_6_6", Mesh));

            var plain = Description(0f);
            using var nanBias = DesktopPipelineBuilder.GraphicsStream(device2, root, Description(float.NaN), [vertex, pixel]);
            using var zeroBias = DesktopPipelineBuilder.GraphicsStream(device2, root, plain, [vertex, pixel]);
            using var otherTarget = DesktopPipelineBuilder.GraphicsStream(device2, root, plain with { RenderTargetFormats = [B5G6R5Unorm] }, [vertex, pixel]);
            using var meshStream = DesktopPipelineBuilder.GraphicsStream(device2, root, plain with { InputLayout = [] }, [mesh, pixel]);

            Assert.Equal(zeroBias.Key, nanBias.Key);
            Assert.NotEqual(zeroBias.Key, otherTarget.Key);
            Assert.Equal(0, StoreAndLoad(device2, nanBias));
            Assert.Equal(0, StoreAndLoad(device2, meshStream));

            DesktopDevice.Release(root);
            DesktopDevice.Release(device2);
        }
        finally
        {
            folder.Delete(recursive: true);
        }
    }

    /// <summary>Builds the stream's pipeline, stores it in a new library under its key and loads it back with the same stream.</summary>
    private static int StoreAndLoad(nint device2, DesktopStream stream)
    {
        var iid = DesktopDevice.PipelineStateIid;
        nint pipeline, library, loaded;
        Assert.Equal(0, stream.Create(device2, &iid, &pipeline));
        Assert.Equal(0, DesktopDevice.CreatePipelineLibrary(device2, null, 0, &library));
        Assert.Equal(0, DesktopPipelineLibrary.Store(library, stream.Key, pipeline));

        int hr = DesktopPipelineLibrary.Load(library, stream.Key, stream.Describe(), &iid, &loaded);

        if (hr == 0)
        {
            DesktopDevice.Release(loaded);
        }

        DesktopDevice.Release(pipeline);
        DesktopDevice.Release(library);

        return hr;
    }

    private static GraphicsPipelineDescription Description(float slope)
    {
        var rasterizer = DesktopStateBytes.DefaultRasterizer();
        BinaryPrimitives.WriteSingleLittleEndian(rasterizer.AsSpan(DesktopStateBytes.SlopeScaledDepthBiasField), slope);

        return GraphicsPipelineDescription.Defaults(0) with
        {
            Rasterizer = rasterizer,
            InputLayout = [new VertexElement("POSITION", 0, Rgba32Float, 0, 0, 0, 0)],
            RenderTargetFormats = [Rgba8Unorm],
            DepthStencilFormat = CachedStateMapping.DepthFormat,
        };
    }

    private static (nint Device2, nint Root) DesktopObjects(nint device)
    {
        var iid = DesktopDevice.Device2Iid;
        nint device2, root;

        Assert.Equal(0, DesktopDevice.QueryInterface(device, &iid, &device2));
        Assert.Equal(0, BindlessRoot.Create(device, &root));

        return (device2, root);
    }

    private static byte[] Compile(string folder, string profile, string source)
    {
        string input = Path.Combine(folder, profile + ".hlsl");
        string output = Path.Combine(folder, profile + ".dxil");
        File.WriteAllText(input, source);

        using var dxc = Process.Start(new ProcessStartInfo(PipelineFactAttribute.Dxc, ["-T", profile, "-E", "main", "-Fo", output, input]) { RedirectStandardError = true })!;
        string errors = dxc.StandardError.ReadToEnd();
        dxc.WaitForExit();
        Assert.True(dxc.ExitCode == 0, errors);

        return File.ReadAllBytes(output);
    }
}
