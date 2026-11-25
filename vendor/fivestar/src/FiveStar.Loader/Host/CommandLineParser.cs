// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using FiveStar.Loader.Loading;
using FiveStar.Loader.Shims.Network;

namespace FiveStar.Loader.Host;

/// <summary>Parses the fivestar-loader command line; <see cref="HostUsage"/> lists the options.</summary>
internal sealed class CommandLineParser
{
    private const double BytesPerGigabyte = 1L << 30;

    private readonly string[] _args;
    private readonly HostOptions _options = new();
    private int _index;

    public CommandLineParser(string[] args)
    {
        _args = args;
    }

    /// <summary>Parses every argument; stops early at --help and --list-safe-calls.</summary>
    public HostOptions Parse()
    {
        for (_index = 0; _index < _args.Length; _index++)
        {
            if (!Apply(_args[_index]))
            {
                break;
            }
        }

        return _options;
    }

    /// <summary>Applies one option; false ends parsing.</summary>
    private bool Apply(string option)
    {
        switch (option)
        {
            case "--load":
                return Add(_options.Loads);
            case "--call":
                return Add(_options.Calls);
            case "--manual":
                return AddManual();
            case "--system-crt":
                return SetSystemCrt();
            case "--search":
                return AddSearchDirectory();
            case "--unresolved":
                return SetUnresolved();
            case "--no-entry":
                return DisableEntryPoints();
            case "--no-shim":
                return Add(_options.DisabledShims);
            case "--no-exports":
                return DisableExportListing();
            case "--unload":
                return SetUnload();
            case "--work-dir":
                return SetWorkDirectory();
            case "--log":
                return SetLogFile();
            case "--main":
                return SetMainModule();
            case "--patch-peb":
                return SetPatchPeb();
            case "--package-root":
                return SetPackageRoot();
            case "--save-dir":
                return SetSaveDirectory();
            case "--gdk-abi":
                return SetGdkAbi();
            case "--device-type":
                return SetDeviceType();
            case "--patches":
                return AddFullPath(_options.Patches);
            case "--mods":
                return AddFullPath(_options.ModFolders);
            case "--input":
                return SetInputConfig();
            case "-v":
                return SetVerbose();
            case "--list-safe-calls":
                return StopWithListSafeCalls();
            case "-h":
            case "--help":
                return StopWithHelp();
            default:
                return ApplyOther(option);
        }
    }

    /// <summary>Options handled past the main switch, to keep <see cref="Apply"/> within the line limit.</summary>
    private bool ApplyOther(string option)
    {
        switch (option)
        {
            case "--shader-dir":
                return SetShaderDirectory();
            case "--translated-dir":
                return SetTranslatedDirectory();
            case "--classify":
                return SetClassifyInput();
            case "--classify-out":
                return SetClassifyOutput();
            case "--run":
                return SetRunMain();
            case "--run-timeout":
                return SetRunTimeout();
            case "--input-script":
                return SetInputScript();
            case "--sample-threads":
                return SetSampleThreads();
            case "--profile-threads" or "--profile-managed":
                return SetProfile(option);
            case "--trace-module":
                return Add(_options.TracedModules);
            case "--trace-system":
                return SetTraceSystem();
            case "--ffx":
                return SetFidelityFxLibrary();
            case "--d3d12-debug" or "--d3d12-gpu-validation" or "--d3d12-dred":
                return SetD3D12Debug(option);
            case "--d3d12-skip":
                return Add(_options.D3D12Skips);
            case "--d3d12-trace-queues":
                return SetTraceQueues();
            case "--d3d12-breadcrumbs":
                return SetBreadcrumbs();
            case "--d3d12-trace-draws" or "--d3d12-trace-pipeline" or "--d3d12-trace-records" or "--d3d12-dump-targets" or "--probe-pipeline" or "--census-passes" or "--hold-passes" or "--stub-passes":
                return SetTraceDraws(option);
            case "--capture-dir":
                return SetCaptureDirectory();
            case "--capture-presents":
                return SetCapturePresents();
            case "--present-test":
                return SetPresentTest();
            case "--raytracing" or "--raytracing-gpu" or "--no-rt-stand-ins":
                return SetRayTracing(option);
            case "--xdsp-convolution":
                return SetXDspConvolution();
            case "--network":
                return SetNetworkMode();
            case "--network-level":
                return SetNetworkLevel();
            case "--cpu":
                return SetCpu();
            default:
                return ApplyCacheAndStorage(option);
        }
    }

    /// <summary>The pipeline-cache, warm-up and DirectStorage options, <c>--watch-transient</c>, <c>--screens</c> and the audio capture.</summary>
    private bool ApplyCacheAndStorage(string option)
    {
        switch (option)
        {
            case "--watch-transient" or "--watch-hang" or "--skip-compute-chains" or "--empty-indirect-after-skip" or "--retire-waits-all-queues" or "--no-split-at-waits" or "--user-data-constants" or "--buffer-records-heap-index" or "--typed-buffer-views" or "--check-null-addresses" or "--no-window-guard" or "--trace-pool-stores" or "--watch-pool-pages":
                return SetWatch(option);
            case "--screens":
                return SetScreensFile();
            case "--audio-capture":
                return SetAudioCapture();
            case "--record-audio":
                return SetRecordAudio();
            case "--desktop-timers":
                _options.DesktopTimers = true;

                return true;
            case "--bcpack-missing":
                return SetBcPackMissing();
            case "--bcpack-tables":
                return SetBcPackTables();
            case "--pipeline-cache":
                return SetPipelineCache();
            case "--no-pipeline-cache":
                return DisablePipelineCache();
            case "--warm-pipelines":
                return SetWarmSource();
            case "--warm-jobs" or "--warm-step" or "--warm-processes" or "--warm-shard":
                return SetWarmSplit(option);
            case "--warm-match":
                return Add(_options.WarmMatches);
            case "--eager-pipelines":
                _options.EagerPipelines = true;

                return true;
            case "--warm-used-only":
                _options.WarmUsedOnly = true;

                return true;
            case "--program-cache":
                _options.ProgramCache = Path.GetFullPath(Next());

                return true;
            case "--no-on-demand":
                _options.NoOnDemand = true;

                return true;
            case "--dxc":
                _options.Dxc = Path.GetFullPath(Next());

                return true;
            case "--shader-jobs" or "--loop-caps" or "--force-cull-none" or "--skip-pipelines" or "--mirror-table-records" or "--plain-supersteps" or "--host-pipelines" or "--superstep-round-budget" or "--fast-waves":
                return SetShaderJobs(option);
            case "--shader-backend":
                return SetShaderBackend();
            default:
                return ApplyMemory(option);
        }
    }

    private bool SetPipelineCache()
    {
        _options.PipelineCache = Path.GetFullPath(Next());

        return true;
    }

    private bool DisablePipelineCache()
    {
        _options.NoPipelineCache = true;

        return true;
    }

    private bool SetWarmSource()
    {
        _options.WarmSource = Path.GetFullPath(Next());

        return true;
    }

    /// <summary>--warm-jobs n, --warm-step k, --warm-processes n, or --warm-shard i/n (set by a split warm-up's parent).</summary>
    private bool SetWarmSplit(string option)
    {
        if (option == "--warm-jobs")
        {
            return SetWarmJobs();
        }

        if (option == "--warm-step")
        {
            return SetWarmStep();
        }

        if (option == "--warm-processes")
        {
            _options.WarmProcesses = int.Parse(Next(), CultureInfo.InvariantCulture);

            return true;
        }

        _options.WarmShard = WarmShards.ParseShard(Next()) ?? throw new CommandLineException("--warm-shard takes i/n with 0 <= i < n");

        return true;
    }

    private bool SetWarmJobs()
    {
        _options.WarmJobs = int.Parse(Next(), CultureInfo.InvariantCulture);

        return true;
    }

    /// <summary>
    /// --shader-jobs N, --loop-caps victory|default|compute,geometry,pixel, --plain-supersteps, --fast-waves, --host-pipelines (programs made on demand) or
    /// --force-cull-none text or --skip-pipelines text,....
    /// </summary>
    private bool SetShaderJobs(string option)
    {
        if (option == "--mirror-table-records")
        {
            return SetMirrorTableRecords(Next());
        }

        if (option == "--skip-pipelines")
        {
            _options.SkipPipelines = Shims.D3D12X.Pipelines.SkippedPipelines.Parse(Next());

            return true;
        }

        if (option == "--plain-supersteps")
        {
            _options.SuperstepCarry = false;

            return true;
        }

        if (option == "--fast-waves")
        {
            _options.FastWaves = true;

            return true;
        }

        if (option == "--host-pipelines")
        {
            _options.HostPipelines = true;

            return true;
        }

        if (option == "--force-cull-none")
        {
            _options.ForceCullNone = Next();

            return true;
        }

        if (option == "--superstep-round-budget")
        {
            _options.SuperstepRoundsPerStop = int.Parse(Next(), CultureInfo.InvariantCulture);

            return true;
        }

        if (option == "--loop-caps")
        {
            _options.LoopCaps = FiveStar.Shaders.Translation.Core.LoopCaps.Parse(Next()) ?? throw new ArgumentException("--loop-caps takes victory, default or compute,geometry,pixel");

            return true;
        }

        _options.ShaderJobs = int.Parse(Next(), CultureInfo.InvariantCulture);

        return true;
    }

    /// <summary>--mirror-table-records reach, full or a count.</summary>
    private bool SetMirrorTableRecords(string value)
    {
        _options.MirrorTableReach = value.Equals("reach", StringComparison.OrdinalIgnoreCase);

        if (!_options.MirrorTableReach)
        {
            _options.MirrorTableRecords = Shims.D3D12X.Binding.DescriptorWindow.ParseTableRecords(value) ?? throw new ArgumentException("--mirror-table-records takes reach, full or a count from 1 to 1024");
        }

        return true;
    }

    private bool SetCompileLimit(string option)
    {
        var value = double.Parse(Next(), CultureInfo.InvariantCulture);

        if (option == "--max-driver-compiles")
        {
            _options.MaxDriverCompiles = (int)value;
        }
        else
        {
            _options.CompileCeilingBytes = (long)(value * BytesPerGigabyte);
        }

        return true;
    }

    private bool SetShaderBackend()
    {
        var backend = Next();

        if (backend is not ("dxil" or "dxc"))
        {
            throw new CommandLineException($"--shader-backend {backend}: expected dxil or dxc");
        }

        _options.DxcBackend = backend == "dxc";

        return true;
    }

    private bool SetWarmStep()
    {
        _options.WarmStep = int.Parse(Next(), CultureInfo.InvariantCulture);

        return true;
    }

    private string Next()
    {
        if (_index + 1 >= _args.Length)
        {
            throw new CommandLineException($"{_args[_index]} needs a value");
        }

        _index++;

        return _args[_index];
    }

    private bool Add(List<string> values)
    {
        values.Add(Next());

        return true;
    }

    private bool AddFullPath(List<string> values)
    {
        values.Add(Path.GetFullPath(Next()));

        return true;
    }

    private bool AddManual()
    {
        foreach (string module in Next().Split(',', StringSplitOptions.RemoveEmptyEntries))
        {
            _options.ManualModules.Add(ModuleNames.NormalizeImport(module));
        }

        return true;
    }

    private bool SetSystemCrt()
    {
        _options.SystemCrt = true;

        return true;
    }

    private bool AddSearchDirectory()
    {
        _options.SearchDirectories.Add(Path.GetFullPath(Next()));

        return true;
    }

    private bool SetUnresolved()
    {
        _options.Unresolved = Enum.Parse<UnresolvedMode>(Next(), true);

        return true;
    }

    private bool DisableEntryPoints()
    {
        _options.CallEntryPoints = false;

        return true;
    }

    private bool DisableExportListing()
    {
        _options.ListExports = false;

        return true;
    }

    private bool SetUnload()
    {
        _options.Unload = true;

        return true;
    }

    private bool SetWorkDirectory()
    {
        _options.WorkDirectory = Path.GetFullPath(Next());

        return true;
    }

    private bool SetLogFile()
    {
        _options.LogFile = Next();

        return true;
    }

    private bool SetMainModule()
    {
        _options.MainModule = Path.GetFileName(Next()).ToLowerInvariant();

        return true;
    }

    private bool SetPatchPeb()
    {
        _options.PatchPeb = true;

        return true;
    }

    private bool SetPackageRoot()
    {
        _options.PackageRoot = Path.GetFullPath(Next());

        return true;
    }

    private bool SetSaveDirectory()
    {
        _options.SaveDirectory = Path.GetFullPath(Next());

        return true;
    }

    private bool SetShaderDirectory()
    {
        _options.ShaderDirectory = Path.GetFullPath(Next());

        return true;
    }

    private bool SetTranslatedDirectory()
    {
        _options.TranslatedDirectory = Path.GetFullPath(Next());

        return true;
    }

    private bool SetGdkAbi()
    {
        _options.GdkAbiTable = Next();

        return true;
    }

    private bool SetDeviceType()
    {
        _options.DeviceType = uint.Parse(Next(), CultureInfo.InvariantCulture);

        return true;
    }

    private bool SetInputConfig()
    {
        _options.InputConfig = Path.GetFullPath(Next());

        return true;
    }

    private bool SetInputScript()
    {
        _options.InputScript = Path.GetFullPath(Next());

        return true;
    }

    private bool SetScreensFile()
    {
        _options.ScreensFile = Path.GetFullPath(Next());

        return true;
    }

    private bool SetRunMain()
    {
        _options.RunMain = true;

        return true;
    }

    private bool SetRunTimeout()
    {
        _options.RunTimeout = TimeSpan.FromSeconds(double.Parse(Next(), CultureInfo.InvariantCulture));

        return true;
    }

    private bool SetSampleThreads()
    {
        _options.SampleThreads = TimeSpan.FromSeconds(double.Parse(Next(), CultureInfo.InvariantCulture));

        return true;
    }

    private bool SetAudioCapture()
    {
        _options.AudioCapture = Path.GetFullPath(Next());

        return true;
    }

    /// <summary>The memory map and the memory limits.</summary>
    private bool ApplyMemory(string option)
    {
        switch (option)
        {
            case "--memory-map":
                return SetMemoryMap();
            case "--min-free-memory":
                return SetMemoryFloor();
            case "--max-commit":
                return SetCommitCeiling();
            case "--section-heap-budget":
                _options.SectionHeapBudget = Gigabytes(Next());

                return true;
            case "--in-place-window":
                _options.InPlaceWindowMegabytes = ulong.Parse(Next(), CultureInfo.InvariantCulture);

                return true;
            case "--evict-windows":
                _options.EvictWindows = true;

                return true;
            case "--reserve-gpu-system-memory":
                _options.ReservedSystemMemory = Gigabytes(Next());

                return true;
            case "--max-driver-compiles" or "--compile-ceiling":
                return SetCompileLimit(option);
            default:
                return ApplyResources(option);
        }
    }

    /// <summary>Texture, view and pipeline-build options.</summary>
    private bool ApplyResources(string option)
    {
        switch (option)
        {
            case "--placed-textures":
                _options.PlacedTextures = true;

                return true;
            case "--typed-image-views":
                _options.TypedImageViews = true;

                return true;
            case "--depth-twins":
                _options.DepthTwins = true;

                return true;
            case "--typeless-targets":
                _options.TypelessTargets = true;

                return true;
            case "--committed-textures":
                _options.PlacedTextures = false;

                return true;
            case "--prebuild-jobs":
                _options.PrebuildJobs = int.Parse(Next(), CultureInfo.InvariantCulture);

                return true;
            case "--wait-pipelines":
                _options.WaitPipelines = Next();

                return true;
            case "--kept-aliases":
                _options.KeptAliases = Math.Max(0, int.Parse(Next(), CultureInfo.InvariantCulture));

                return true;
            default:
                throw new CommandLineException($"unknown argument {option}");
        }
    }

    private bool SetMemoryFloor()
    {
        _options.MinimumFreeMemory = Gigabytes(Next());

        return true;
    }

    private bool SetCommitCeiling()
    {
        _options.MaximumCommit = Gigabytes(Next());

        return true;
    }

    private static ulong Gigabytes(string text)
    {
        return (ulong)(double.Parse(text, CultureInfo.InvariantCulture) * BytesPerGigabyte);
    }

    private bool SetMemoryMap()
    {
        _options.MemoryMap = TimeSpan.FromSeconds(double.Parse(Next(), CultureInfo.InvariantCulture));

        return true;
    }

    /// <summary>--profile-threads, or --profile-managed &lt;start&gt;[:&lt;seconds&gt;].</summary>
    private bool SetProfile(string option)
    {
        if (option == "--profile-threads")
        {
            _options.ProfileThreads = true;

            return true;
        }

        string value = Next();

        if (!Diagnostics.ManagedProfile.TryParse(value, out var start, out var length))
        {
            throw new CommandLineException($"--profile-managed {value}: expected <start seconds>[:<seconds>]");
        }

        (_options.ProfileManagedStart, _options.ProfileManagedLength) = (start, length);

        return true;
    }

    private bool SetTraceSystem()
    {
        _options.TraceSystemCalls = true;

        return true;
    }

    private bool SetFidelityFxLibrary()
    {
        _options.FidelityFxLibrary = Path.GetFullPath(Next());

        return true;
    }

    /// <summary>--d3d12-debug, --d3d12-gpu-validation, which implies it, or --d3d12-dred (DRED without the debug layer).</summary>
    private bool SetD3D12Debug(string option)
    {
        if (option == "--d3d12-dred")
        {
            _options.D3D12Dred = true;

            return true;
        }

        _options.D3D12Debug = true;
        _options.D3D12GpuValidation |= option == "--d3d12-gpu-validation";

        return true;
    }

    private bool SetTraceQueues()
    {
        _options.D3D12TraceQueues = true;

        return true;
    }

    private bool SetBreadcrumbs()
    {
        _options.D3D12Breadcrumbs = true;

        return true;
    }

    /// <summary><c>--watch-transient</c>, <c>--watch-hang</c> or <c>--skip-compute-chains</c>.</summary>
    private bool SetWatch(string option)
    {
        if (option == "--watch-hang")
        {
            _options.WatchHang = true;
        }
        else if (option == "--trace-pool-stores")
        {
            _options.TracePoolStores = true;
        }
        else if (option == "--retire-waits-all-queues")
        {
            _options.RetireWaitsAllQueues = true;
        }
        else if (option == "--watch-pool-pages")
        {
            _options.WatchPoolPages = true;
        }
        else if (option == "--no-split-at-waits")
        {
            _options.SplitAtWaits = false;
        }
        else if (option == "--user-data-constants")
        {
            _options.UserDataConstants = true;
        }
        else if (option == "--buffer-records-heap-index")
        {
            _options.BufferRecordsHeapIndex = true;
        }
        else if (option == "--typed-buffer-views")
        {
            _options.TypedBufferViews = true;
        }
        else if (option == "--check-null-addresses")
        {
            _options.CheckNullAddresses = true;
        }
        else if (option == "--no-window-guard")
        {
            _options.NoWindowGuard = true;
        }
        else if (option == "--skip-compute-chains")
        {
            _options.SkipComputeChains = true;
        }
        else if (option == "--empty-indirect-after-skip")
        {
            _options.EmptyIndirectAfterSkip = true;
        }
        else
        {
            _options.WatchTransient = true;
        }

        return true;
    }

    /// <summary><c>--d3d12-trace-draws</c>, or <c>--d3d12-trace-pipeline &lt;text&gt;</c>, which also names pipelines to trace.</summary>
    /// <summary>The draw trace options, and --probe-pipeline &lt;text&gt; (no trace).</summary>
    private bool SetTraceDraws(string option)
    {
        if (option == "--probe-pipeline")
        {
            _options.ProbePipeline = Next();

            return true;
        }

        if (option is "--hold-passes" or "--stub-passes")
        {
            return SetPassRules(option);
        }

        _options.D3D12TraceDraws = true;

        if (option == "--census-passes")
        {
            _options.CensusPasses = [.. Next().Split(',', StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries)];
        }

        if (option == "--d3d12-trace-pipeline")
        {
            _options.D3D12TracePipeline = Next();
        }

        if (option == "--d3d12-trace-records")
        {
            _options.D3D12TraceRecords = Shims.D3D12X.DebugLayer.TraceRecords.Parse(Next());
        }

        if (option == "--d3d12-dump-targets")
        {
            _options.D3D12DumpTargets = true;
        }

        return true;
    }

    /// <summary><c>--hold-passes consumer=producer|...;...</c> or <c>--stub-passes pass=r,g,b,a;...</c> (no trace).</summary>
    private bool SetPassRules(string option)
    {
        if (option == "--hold-passes")
        {
            _options.HoldPasses = Next();
        }
        else
        {
            _options.StubPasses = Next();
        }

        return true;
    }

    private bool SetCaptureDirectory()
    {
        _options.CaptureDirectory = Path.GetFullPath(Next());

        return true;
    }

    private bool SetCapturePresents()
    {
        _options.CapturePresents.Clear();

        foreach (string present in Next().Split(',', StringSplitOptions.RemoveEmptyEntries))
        {
            _options.CapturePresents.Add(long.Parse(present, CultureInfo.InvariantCulture));
        }

        return true;
    }

    private bool SetPresentTest()
    {
        _options.PresentTest = true;

        return true;
    }

    /// <summary>--raytracing (the DLL's CPU paths), --raytracing-gpu (the inventory of its GPU builder) or --no-rt-stand-ins.</summary>
    private bool SetRayTracing(string option)
    {
        if (option == "--no-rt-stand-ins")
        {
            _options.NoRtStandIns = true;

            return true;
        }

        if (option == "--raytracing-gpu")
        {
            _options.RayTracingGpuInventory = true;
        }
        else
        {
            _options.RayTracing = true;
        }

        return true;
    }

    /// <summary>--network offline | sealed-online.</summary>
    private bool SetNetworkMode()
    {
        string value = Next();

        _options.NetworkMode = value switch
        {
            "offline" => NetworkMode.Offline,
            "sealed-online" => NetworkMode.SealedOnline,
            _ => throw new CommandLineException($"--network {value}: expected offline or sealed-online"),
        };

        return true;
    }

    /// <summary>--network-level: a level name or its XNetworkingConnectivityLevelHint value (0..4).</summary>
    private bool SetNetworkLevel()
    {
        string value = Next();

        if (!Enum.TryParse<ConnectivityLevel>(value, true, out var level) || !Enum.IsDefined(level))
        {
            throw new CommandLineException($"--network-level {value}: expected 0..4 or Unknown, None, LocalAccess, InternetAccess, ConstrainedInternetAccess");
        }

        _options.NetworkLevel = level;

        return true;
    }

    /// <summary>--cpu host | intel.</summary>
    private bool SetCpu()
    {
        string value = Next();

        _options.AssumeIntelCpu = value switch
        {
            "host" => false,
            "intel" => true,
            _ => throw new CommandLineException($"--cpu {value}: expected host or intel"),
        };

        return true;
    }

    /// <summary>--bcpack-missing placeholder | fail.</summary>
    private bool SetBcPackMissing()
    {
        string value = Next();

        _options.FailMissingBcPack = value switch
        {
            "placeholder" => false,
            "fail" => true,
            _ => throw new CommandLineException($"--bcpack-missing {value}: expected placeholder or fail"),
        };

        return true;
    }

    private bool SetBcPackTables()
    {
        _options.BcPackTables = Path.GetFullPath(Next());

        return true;
    }

    private bool SetXDspConvolution()
    {
        _options.XDspConvolution = true;

        return true;
    }

    private bool SetRecordAudio()
    {
        _options.RecordAudioProcess = int.Parse(Next(), CultureInfo.InvariantCulture);
        _options.AudioCapture = Path.GetFullPath(Next());

        return true;
    }

    private bool SetVerbose()
    {
        _options.Verbose = true;

        return true;
    }

    private bool SetClassifyInput()
    {
        _options.ClassifyInput = Path.GetFullPath(Next());

        return true;
    }

    private bool SetClassifyOutput()
    {
        _options.ClassifyOutput = Path.GetFullPath(Next());

        return true;
    }

    private bool StopWithListSafeCalls()
    {
        _options.ListSafeCalls = true;

        return false;
    }

    private bool StopWithHelp()
    {
        _options.Help = true;

        return false;
    }
}
