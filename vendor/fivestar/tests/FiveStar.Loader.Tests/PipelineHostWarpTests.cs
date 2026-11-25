// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Graphics;
using FiveStar.Loader.Shims.D3D12X.Pipelines;
using FiveStar.Loader.Tests.Support;
using FiveStar.Shaders.Compilation;
using FiveStar.Shaders.Hardware;
using Xunit;

namespace FiveStar.Loader.Tests;

/// <summary>
/// A mesh + pixel pipeline as the loader streams it (<see cref="DesktopPipelineBuilder.GraphicsStream"/>) builds in a
/// pipeline host on WARP as it does in the loader.
/// </summary>
public sealed unsafe class PipelineHostWarpTests
{
    private const int Rgba8Unorm = 28;

    private const string MeshShader = """
        struct V { float4 pos : SV_Position; float4 color : COLOR0; };
        [outputtopology("triangle")]
        [numthreads(3, 1, 1)]
        void main(uint i : SV_GroupThreadID, out vertices V verts[3], out indices uint3 tris[1]) {
          SetMeshOutputCounts(3, 1);
          verts[i].pos = float4(i, 0, 0, 1);
          verts[i].color = float4(1, 0, 0, 1);
          if (i == 0) tris[0] = uint3(0, 1, 2);
        }
        """;

    private const string PixelShader = """
        float4 main(float4 pos : SV_Position, float4 color : COLOR0) : SV_Target0 { return color; }
        """;

    [HostPipelineFact]
    public void A_mesh_and_pixel_stream_builds_in_the_host_as_in_the_loader()
    {
        var work = Directory.CreateTempSubdirectory("fivestar-pipeline-host-");

        try
        {
            using var device = GpuDevice.CreateWarp(debugLayer: false);
            var programs = new[]
            {
                new Shims.D3D12X.Pipelines.TranslatedProgram("ms", HardwareStage.Ngg, Compile(work.FullName, "ms", MeshShader)),
                new Shims.D3D12X.Pipelines.TranslatedProgram("ps", HardwareStage.Pixel, Compile(work.FullName, "ps", PixelShader)),
            };
            var iid = DesktopDevice.Device2Iid;
            var pipelineIid = DesktopDevice.PipelineStateIid;
            nint device2, root, here;

            Assert.Equal(0, DesktopDevice.QueryInterface(device.Device.NativePointer, &iid, &device2));
            Assert.Equal(0, BindlessRoot.Create(device.Device.NativePointer, &root));

            var description = GraphicsPipelineDescription.Defaults(0) with { RenderTargetFormats = [Rgba8Unorm] };
            using var stream = DesktopPipelineBuilder.GraphicsStream(device2, root, description, programs);

            Assert.Equal(0, stream.Create(device2, &pipelineIid, &here));

            var host = new PipelineHost(Path.Combine(AppContext.BaseDirectory, "fivestar-loader.exe"));
            var request = new PipelineHostRequest(device.Device.AdapterLuid, BindlessRoot.Serialized()!, 0);

            var result = host.Build(request, stream, 2L << 30, TimeSpan.FromSeconds(120));

            Assert.True(result.Outcome == PipelineHostOutcome.Built, $"{result} 0x{result.HResult:x8}");

            foreach (var unknown in new[] { here, root, device2 })
            {
                DesktopDevice.Release(unknown);
            }
        }
        finally
        {
            work.Delete(recursive: true);
        }
    }

    private static byte[] Compile(string folder, string name, string hlsl)
    {
        var compiler = new DxcCompiler(DxcOptions.Defaults(PipelineFactAttribute.Dxc, retry: false));
        var request = new CompileRequest(Path.Combine(folder, name + ".hlsl"), hlsl, name, DisableOptimizations: false);
        File.WriteAllText(request.HlslPath, hlsl);
        var outcome = compiler.Compile(request, TimeSpan.FromSeconds(60));
        Assert.True(outcome.Succeeded, outcome.Error);

        return File.ReadAllBytes(request.DxilPath);
    }
}
