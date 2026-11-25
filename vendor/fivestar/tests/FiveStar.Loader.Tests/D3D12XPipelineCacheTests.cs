// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Diagnostics;
using FiveStar.Graphics;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Shims.D3D12X.Pipelines;
using FiveStar.Loader.Tests.Support;
using FiveStar.Shaders.Hardware;
using Xunit;

namespace FiveStar.Loader.Tests;

/// <summary>
/// The pipeline disk cache on D3D12 WARP: a pipeline one process built is loaded by the next at once, without a
/// stand-in, and segment files of another build are deleted.
/// </summary>
public sealed unsafe class D3D12XPipelineCacheTests
{
    private const string Compute = "groupshared uint g[8]; [numthreads(8, 1, 1)] void main(uint i : SV_GroupIndex) { g[i] = i; }";

    [DxcFact]
    public void A_pipeline_built_by_one_process_is_loaded_by_the_next_without_a_stand_in()
    {
        var folder = Directory.CreateTempSubdirectory("fivestar-psolib-");

        try
        {
            using var device = GpuDevice.CreateWarp(debugLayer: false);
            var (device2, root) = DesktopObjects(device.Device.NativePointer);
            var program = new TranslatedProgram("cs", HardwareStage.Compute, Compile(folder.FullName));
            var description = new ComputePipelineDescription(0, new ShaderSource(ShaderSourceKind.CacheCode, new byte[24]));

            var (first, firstSummary) = CreateOnce(folder.FullName, device2, root, program, description, TimeSpan.FromMinutes(1));
            var (second, secondSummary) = CreateOnce(folder.FullName, device2, root, program, description, TimeSpan.Zero);

            Assert.True(first, firstSummary);
            Assert.True(second, secondSummary);
            Assert.Contains("1 stored", firstSummary, StringComparison.Ordinal);
            Assert.Contains("1 loaded", secondSummary, StringComparison.Ordinal);
            DesktopDevice.Release(root);
            DesktopDevice.Release(device2);
        }
        finally
        {
            folder.Delete(recursive: true);
        }
    }

    [DxcFact]
    public void A_journal_pipeline_prebuilt_at_device_creation_is_loaded_by_the_game_without_a_stand_in()
    {
        var folder = Directory.CreateTempSubdirectory("fivestar-prebuild-");

        try
        {
            using var device = GpuDevice.CreateWarp(debugLayer: false);
            var (device2, root) = DesktopObjects(device.Device.NativePointer);
            var program = new TranslatedProgram("cs", HardwareStage.Compute, Compile(folder.FullName));
            var description = new ComputePipelineDescription(0, new ShaderSource(ShaderSourceKind.CacheCode, new byte[24]));
            var target = new WarmTarget(0, null, description);

            var log = new LoaderLog(new StringWriter(), null, false);
            var warnings = new OnceLog(log);
            var builds = new PipelineBuildQueue(1, PipelineBuildQueue.AnyTime);
            builds.Start();

            var cache = PipelineLibraryCache.Open(folder.FullName, device2, log)!;
            Assert.True(cache.WaitForSegments(TimeSpan.FromMinutes(1)));
            var factory = new PipelineFactory(device2, root, new FixedProgramLibrary(program), new ProgramSignatures(folder.FullName), warnings, null, new StandInPipelines(TimeSpan.Zero, builds, builds, warnings), cache);
            var prebuild = new JournalPrebuild(factory, cache);

            var first = prebuild.Build(target);
            var again = prebuild.Build(target);
            cache.Dispose();

            // The game's next creation, with no time for a build, finds it cached.
            var (direct, summary) = CreateOnce(folder.FullName, device2, root, program, description, TimeSpan.Zero);

            Assert.Equal((PrebuildOutcome.Built, PrebuildOutcome.Cached), (first, again));
            Assert.True(direct, summary);
            DesktopDevice.Release(root);
            DesktopDevice.Release(device2);
        }
        finally
        {
            folder.Delete(recursive: true);
        }
    }

    [Fact]
    public void A_segment_of_another_build_is_deleted()
    {
        var folder = Directory.CreateTempSubdirectory("fivestar-psolib-");

        try
        {
            var foreign = Path.Combine(folder.FullName, "old" + PipelineLibraryFile.Extension);
            File.WriteAllBytes(foreign, new byte[0x100]);
            using var device = GpuDevice.CreateWarp(debugLayer: false);
            var (device2, root) = DesktopObjects(device.Device.NativePointer);

            using (PipelineLibraryCache.Open(folder.FullName, device2, new LoaderLog(new StringWriter(), null, false)))
            {
                Assert.False(File.Exists(foreign));
            }

            DesktopDevice.Release(root);
            DesktopDevice.Release(device2);
        }
        finally
        {
            folder.Delete(recursive: true);
        }
    }

    /// <summary>
    /// One process's creation through a factory with the cache; true when the game got the desktop pipeline itself
    /// (a zero budget gives every build a stand-in, so only a cache hit returns the pipeline).
    /// </summary>
    private static (bool Direct, string Summary) CreateOnce(
        string folder, nint device2, nint root, TranslatedProgram program, ComputePipelineDescription description, TimeSpan budget)
    {
        var log = new LoaderLog(new StringWriter(), null, false);
        var warnings = new OnceLog(log);
        var builds = new PipelineBuildQueue(1, PipelineBuildQueue.AnyTime);
        builds.Start();

        var standIns = new StandInPipelines(budget, builds, builds, warnings);
        var cache = PipelineLibraryCache.Open(folder, device2, log)!;
        Assert.True(cache.WaitForSegments(TimeSpan.FromMinutes(1)));
        var factory = new PipelineFactory(device2, root, new FixedProgramLibrary(program), new ProgramSignatures(folder), warnings, null, standIns, cache);
        var iid = DesktopDevice.PipelineStateIid;
        nint pipeline;

        int hr = factory.Compute(description, &iid, &pipeline);
        var direct = hr == 0 && pipeline != 0 && standIns.Desktop(pipeline, out bool pending) == pipeline && !pending;

        if (direct)
        {
            DesktopDevice.Release(pipeline);
        }

        cache.Dispose();

        return (direct, cache.Summary);
    }

    private static (nint Device2, nint Root) DesktopObjects(nint device)
    {
        var iid = DesktopDevice.Device2Iid;
        nint device2, root;

        Assert.Equal(0, DesktopDevice.QueryInterface(device, &iid, &device2));
        Assert.Equal(0, BindlessRoot.Create(device, &root));

        return (device2, root);
    }

    private static byte[] Compile(string folder)
    {
        string input = Path.Combine(folder, "cs.hlsl");
        string output = Path.Combine(folder, "cs.dxil");
        File.WriteAllText(input, Compute);

        using var dxc = Process.Start(new ProcessStartInfo(PipelineFactAttribute.Dxc, ["-T", "cs_6_6", "-E", "main", "-Fo", output, input]) { RedirectStandardError = true })!;
        string errors = dxc.StandardError.ReadToEnd();
        dxc.WaitForExit();
        Assert.True(dxc.ExitCode == 0, errors);

        return File.ReadAllBytes(output);
    }
}
