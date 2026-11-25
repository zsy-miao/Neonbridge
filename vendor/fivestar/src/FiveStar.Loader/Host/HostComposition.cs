// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Loading;
using FiveStar.Loader.Shims.D3D12X.Pipelines;
using FiveStar.Loader.Shims.Gdk;
using FiveStar.Loader.Shims.Network;

namespace FiveStar.Loader.Host;

/// <summary>Turns the command line and the process environment into the loader's options.</summary>
internal static class HostComposition
{
    private const string WorkFolder = "work";
    private const string DataFolder = "FiveStar";
    private const string GdkDataFolder = "GdkData";

    /// <summary>How long after the pipelines are set up large compiles count as in the game's load window.</summary>
    private static readonly TimeSpan LoadWindowSpan = TimeSpan.FromSeconds(120);

    /// <summary>The shipped MSVC runtime, mapped from the target's directory unless --system-crt is given.</summary>
    public static readonly IReadOnlyList<string> ShippedCrt =
    [
        "msvcp140.dll", "msvcp140_1.dll", "msvcp140_2.dll", "msvcp140_atomic_wait.dll", "msvcp140_codecvt_ids.dll",
        "vcruntime140.dll", "vcruntime140_1.dll", "concrt140.dll", "vccorlib140.dll", "vcomp140.dll",
    ];

    public static LoaderOptions CreateLoaderOptions(HostOptions options)
    {
        var manual = new HashSet<string>(options.ManualModules, StringComparer.OrdinalIgnoreCase);

        if (!options.SystemCrt)
        {
            manual.UnionWith(ShippedCrt);
        }

        return new LoaderOptions(options.SearchDirectories, manual, options.Unresolved, options.CallEntryPoints, WorkDirectory(options), options.MainModule, Environment.ProcessId);
    }

    public static GdkOptions CreateGdkOptions(HostOptions options)
    {
        string dataRoot = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), DataFolder, GdkDataFolder);
        uint deviceType = options.DeviceType ?? SystemService.XboxScarlettAnaconda;

        var network = new NetworkOptions(options.NetworkMode, options.NetworkLevel);

        return new GdkOptions(options.PackageRoot, options.SaveDirectory, dataRoot, Environment.CurrentDirectory, deviceType, Environment.OSVersion.Version, network);
    }

    /// <summary>The binding and draw-diagnostic switches the options set.</summary>
    private static void ApplyBindingSwitches(HostOptions options)
    {
        Shims.D3D12X.DebugLayer.DrawCensus.Passes = options.CensusPasses;
        Shims.D3D12X.Binding.PassControls.Rules = Shims.D3D12X.Binding.PassRules.Parse(options.HoldPasses, options.StubPasses);
        Shims.D3D12X.Binding.BufferRecords.HeapIndexOnly = options.BufferRecordsHeapIndex;
        Shims.D3D12X.Binding.RawBufferViews.Enabled = !options.TypedBufferViews;
        Shims.D3D12X.DebugLayer.NullAddressCheck.Enabled = options.CheckNullAddresses;

        // No acceleration structure is built without --raytracing: programs made on demand miss every BVH node.
        Shims.D3D12X.Binding.BvhMiss.Enabled = !options.RayTracing;
        Shims.D3D12X.Pipelines.RayTracingPrograms.StandIns = !options.RayTracing;
        Shims.D3D12X.Binding.WindowGuard.Enabled = !options.NoWindowGuard;
    }

    /// <summary>The in-place window of <c>--in-place-window</c> (MB), or the default for 0.</summary>
    private static ulong InPlaceWindow(ulong megabytes)
    {
        if (megabytes == 0)
        {
            return Shims.D3D12X.SectionHeaps.WindowBytes;
        }

        return Shims.D3D12X.SectionHeaps.InPlaceWindowOf(megabytes);
    }

    public static PipelineFolders CreatePipelineFolders(HostOptions options)
    {
        CompileBudget.Configure(options.MaxDriverCompiles, options.CompileCeilingBytes);
        Shims.D3D12X.Pipelines.JournalPrebuild.Jobs = options.PrebuildJobs;
        var boot = System.Diagnostics.Stopwatch.StartNew();
        CompileBudget.LoadWindow = () => Diagnostics.GameHangWatch.Loading(boot.Elapsed, LoadWindowSpan);

        Shims.D3D12X.Resources.PlacedTextureHeaps.Enabled = options.PlacedTextures;
        Shims.D3D12X.Resources.GpuTextureRegistry.KeptAliases = options.KeptAliases;
        Shims.D3D12X.Pipelines.PipelineWaits.Current = Shims.D3D12X.Pipelines.PipelineWaits.Parse(options.WaitPipelines);
        Shims.D3D12X.SectionHeaps.InPlaceWindow = InPlaceWindow(options.InPlaceWindowMegabytes);
        Shims.D3D12X.Resources.TypelessTargets.Enabled = options.TypelessTargets;
        Shims.D3D12X.DebugLayer.MeshProbes.Filter = options.ProbePipeline;

        ApplyBindingSwitches(options);
        string? cache = null;

        if (!options.NoPipelineCache)
        {
            cache = options.PipelineCache ?? Path.Combine(WorkDirectory(options), PipelineLibraryCache.FolderName);
        }

        UnbuildablePipelines.OpenIn(cache);
        PipelineHost.Current = options.HostPipelines ? new PipelineHost(Environment.ProcessPath!) : null;
        var folders = new PipelineFolders(options.ShaderDirectory, options.TranslatedDirectory, cache) { EagerBuilds = options.EagerPipelines };

        if (folders.Programs is not null && !options.NoOnDemand)
        {
            var root = options.ProgramCache ?? Path.Combine(WorkDirectory(options), OnDemandStart.FolderName);
            Shims.D3D12X.DebugLayer.PoolStoreTrace.Enabled = options.TracePoolStores;
            Shims.D3D12X.Binding.UserDataConstants.Enabled = options.UserDataConstants;
            Shims.D3D12X.Binding.WindowFeedback.Enabled = options.EvictWindows;
            OnDemandStart.Translation = FiveStar.Shaders.Compilation.CorpusOptions.DefaultTranslation with { Loops = options.LoopCaps, SuperstepCarry = options.SuperstepCarry, RoundsPerStop = options.SuperstepRoundsPerStop, FastWaves = options.FastWaves };
            folders = folders with { OnDemand = OnDemandStart.Begin(root, options.Dxc, options.DxcBackend, options.ShaderJobs, out var message), OnDemandNote = message };
        }

        // Root constants need programs compiled for them: without programs made on demand the user data stays in the ring.
        if (options.UserDataConstants && folders.OnDemand is null)
        {
            Shims.D3D12X.Binding.UserDataConstants.Enabled = false;
            folders = folders with { OnDemandNote = $"{folders.OnDemandNote}; --user-data-constants needs programs made on demand: user data stays a root CBV".TrimStart(';', ' ') };
        }

        // Windows open as programs report them: programs made ahead of time report nothing.
        if (options.EvictWindows && folders.OnDemand is null)
        {
            folders = folders with { OnDemandNote = $"{folders.OnDemandNote}; --evict-windows needs programs made on demand: section windows open as before".TrimStart(';', ' ') };
        }

        if (!folders.Enabled)
        {
            return folders;
        }

        return folders with { Index = ProgramIndexLoad.Start(folders.Programs!) };
    }

    /// <summary>--work-dir, or the work folder beside the loader.</summary>
    private static string WorkDirectory(HostOptions options)
    {
        return options.WorkDirectory ?? Path.Combine(AppContext.BaseDirectory, WorkFolder);
    }

    public static LoaderEnvironment CreateEnvironment()
    {
        return new LoaderEnvironment(Environment.SystemDirectory, TimeProvider.System, Environment.FailFast);
    }
}
