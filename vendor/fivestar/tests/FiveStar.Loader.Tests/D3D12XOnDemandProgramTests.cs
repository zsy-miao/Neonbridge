// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Graphics;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;
using FiveStar.Loader.Shims.D3D12X.Pipelines;
using FiveStar.Loader.Tests.Support;
using FiveStar.Shaders.Cache;
using FiveStar.Shaders.Compilation;
using Xunit;
using Xunit.Abstractions;

namespace FiveStar.Loader.Tests;

/// <summary>
/// Programs made on demand on WARP with a real program: a compute pipeline whose program no translation holds gets a
/// stand-in, the program is translated and compiled in-process (by the DXIL backend, dxc as its fallback) and the
/// stand-in becomes the pipeline; the next "boot"
/// finds the program in the program cache and creates the pipeline at once.
/// </summary>
public sealed unsafe class D3D12XOnDemandProgramTests(ITestOutputHelper output)
{
    private const string ComputePart = "VfxNoisePerlinCS";
    private static readonly TimeSpan MakeLimit = TimeSpan.FromMinutes(2);

    [OnDemandFact]
    public void A_compute_pipeline_waits_for_its_program_made_on_demand_and_the_next_boot_finds_it()
    {
        var work = Directory.CreateTempSubdirectory("fivestar-ondemand-");

        try
        {
            var (programs, name) = CopyProgram(work.FullName);
            var index = ProgramIndex.Load(programs);
            var prefix = Prefix(Path.Combine(programs, name + ".fssm"));
            var cache = Path.Combine(work.FullName, "program-cache");

            using var device = GpuDevice.CreateWarp(debugLayer: false);
            var first = Create(device.Device.NativePointer, index, programs, cache, prefix);
            output.WriteLine($"first: {first.Summary}");
            Assert.True(first.Waited, "the first creation should wait for its program");
            Assert.True(first.Built, first.Summary);

            Assert.Contains("DXIL backend 1, dxc 0", first.Summary, StringComparison.Ordinal);
            Assert.True(File.Exists(Path.Combine(first.Folder, name + ".dxil")));

            var second = Create(device.Device.NativePointer, index, programs, cache, prefix);
            output.WriteLine($"second: {second.Summary}");
            Assert.False(second.Waited, "the second creation should find the program compiled");
            Assert.True(second.Built, second.Summary);
        }
        finally
        {
            work.Delete(recursive: true);
        }
    }

    private static (bool Waited, bool Built, string Folder, string Summary) Create(nint device, ProgramIndex index, string programs, string cache, byte[] prefix)
    {
        var log = new StringWriter();
        var warnings = new OnceLog(new LoaderLog(log, null, false));
        var iid = DesktopDevice.Device2Iid;
        nint device2, root, pipeline;
        Assert.Equal(0, DesktopDevice.QueryInterface(device, &iid, &device2));
        Assert.Equal(0, BindlessRoot.Create(device, &root));

        var adapter = DesktopAdapter.Describe(device2);
        var folder = Path.Combine(cache, "test", adapter.WaveLaneCountMin.ToString(System.Globalization.CultureInfo.InvariantCulture));
        Directory.CreateDirectory(folder);
        FiveStar.Shaders.Translation.RuntimeIncludes.WriteTo(folder);

        var dxc = new DxcLibraryCompiler(DxcOptions.Defaults(OnDemandFactAttribute.Library, retry: false));
        var compiler = new DxilBackendCompiler(dxc.Options, dxc, new InProcessBackend(DxilBackendCompiler.ValidatorBeside(OnDemandFactAttribute.Library)));
        var picker = new ProgramVariantPicker(new FiveStar.Graphics.Programs.ShaderManifest([]), adapter, warnings);

        var producer = new ProgramProducer(programs, folder, picker, compiler, SlowCompiles.Read([], compiler.Options.CompileTimeout));
        var workers = new ProgramWorkers(1, PipelineBuildQueue.AnyTime, PipelineBuildQueue.AnyTime);
        workers.Start();
        var onDemand = new OnDemandPrograms(index, null, producer, workers, warnings, () => ", " + compiler.Summary);

        var builds = new PipelineBuildQueue(1, PipelineBuildQueue.AnyTime);
        builds.Start();
        var standIns = new StandInPipelines(PipelineSlots.CreationBudget, builds, builds, warnings);
        var factory = new PipelineFactory(device2, root, onDemand, new ProgramSignatures(programs), warnings, null, standIns, null) { OnDemand = onDemand };
        var pipelineIid = DesktopDevice.PipelineStateIid;

        var hr = factory.Compute(new ComputePipelineDescription(0, new ShaderSource(ShaderSourceKind.CacheCode, prefix)), &pipelineIid, &pipeline);
        Assert.Equal(HResult.Ok, hr);
        var waited = standIns.Summary.Contains("1 with programs made on demand", StringComparison.Ordinal);
        var created = pipeline;
        var built = SpinWait.SpinUntil(() => standIns.Desktop(created, out _) != 0, MakeLimit);

        return (waited, built, folder, $"{onDemand.Summary}; {standIns.Summary}; {log}");
    }

    /// <summary>Copies the compute program and its container into a programs folder of their own.</summary>
    private static (string Programs, string Name) CopyProgram(string work)
    {
        var source = PipelineFactAttribute.ShaderFolder!;
        var containers = Path.Combine(Path.GetDirectoryName(Path.GetFullPath(source))!, "dxbc");
        var path = Directory.EnumerateFiles(source, $"*{ComputePart}*.fssm").Order(StringComparer.Ordinal).First();
        var name = Path.GetFileNameWithoutExtension(path);
        var programs = Path.Combine(work, "fssm");

        Directory.CreateDirectory(programs);
        Directory.CreateDirectory(Path.Combine(work, "dxbc"));
        File.Copy(path, Path.Combine(programs, name + ".fssm"));
        File.Copy(Path.Combine(containers, name + ".dxbc"), Path.Combine(work, "dxbc", name + ".dxbc"));

        return (programs, name);
    }

    private static byte[] Prefix(string program)
    {
        var bytes = File.ReadAllBytes(program);
        var offset = (int)FiveStar.Shaders.Hardware.FssmHeader.Decode(bytes).CodeOffset!.Value;

        return bytes[offset..(offset + ProgramIndex.PrefixSize)];
    }
}
