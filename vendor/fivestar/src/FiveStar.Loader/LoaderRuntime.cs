// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Calls;
using FiveStar.Loader.Codegen;
using FiveStar.Loader.Cpu;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Loading;
using FiveStar.Loader.Mods;
using FiveStar.Loader.Shims;
using FiveStar.Loader.Shims.Apu;
using FiveStar.Loader.Shims.Bvh;
using FiveStar.Loader.Shims.D3D12X;
using FiveStar.Loader.Shims.D3D12X.Pipelines;
using FiveStar.Loader.Shims.Ffx;
using FiveStar.Loader.Shims.Gdk;
using FiveStar.Loader.Shims.Input;
using FiveStar.Loader.Shims.Network;
using FiveStar.Loader.Shims.Storage;
using FiveStar.Loader.Shims.Xg;

namespace FiveStar.Loader;

/// <summary>
/// The composition root of one loader: the image loader, the shim table and every service the shims call. Creating it
/// makes it the runtime the shim entry points reach through <see cref="ShimContext"/>.
/// </summary>
internal sealed class LoaderRuntime : IDisposable
{
    public LoaderRuntime(LoaderOptions options, GdkOptions gdk, PipelineFolders pipelines, LoaderEnvironment environment, LoaderLog log)
    {
        Log = log;
        Counters = new LoaderCounters();
        CodeHeap = new CodeHeap();
        LastError = new LastErrorBridge(CodeHeap, log);
        Shims = new ShimTable(LastError, CodeHeap);

        Images = new ImageTable();
        Stubs = new UnresolvedStubs(CodeHeap, Images, options.Unresolved, log, environment.FailFast);
        VirtualModules = new VirtualModules(log);
        Patcher = new ImagePatcher(Shims, log);
        Trace = new CallTrace(CodeHeap);

        Cpu = new AmdInstructionHandler(Images, log);
        Faults = new FaultReporter(Images, Trace, CodeHeap, Cpu, log);

        Loader = new ImageLoader(options, Shims, Stubs, Images, CodeHeap, new TlsHostAttacher(options.WorkDirectory, options.ProcessId, log), Patcher, Trace, log);
        Router = new RuntimeLibraryRouter(Loader, VirtualModules, Shims, Images, log);
        Identity = new ProcessIdentity(log);
        ModuleIdentity = new ModuleIdentity(Loader, Router, Images, VirtualModules, Shims, Stubs, LastError, Counters, log, environment.SystemDirectory);

        XMem = new XMemAllocator(Images, log);
        XMemPhysical = new XMemPhysicalMemory(log);
        XMemWorkingSet = new XMemWorkingSet(log);
        Xg = new XgComputers(Stubs, CodeHeap);

        Apu = new ApuHeap(log);
        Audio = new AudioCounters();
        AcpHal = new AcpHalDevice(Apu, Audio, log);
        XApu = new XApuDevice(Audio, log);
        XDsp = new XDspDevice(log);

        D3D12 = new D3D12Forwarder(log);
        D3D12X = new XboxDeviceFactory(D3D12, Stubs, CodeHeap, XMemPhysical, Images, pipelines, log);
        FidelityFx = new FidelityFxBridge(new XboxObjectResolver(D3D12X.Objects), log);
        BvhBuilder = new BvhBuilderService(D3D12X.Objects, Loader, XMemPhysical, log);

        Network = new NetworkSeal(log);
        Gdk = CreateGdk(gdk, environment.Time);
        Overlay = new FileOverlay(Gdk.Config, log);
        Storage = new DStorageFactory(Stubs, CodeHeap, Overlay, new PhysicalPageSink(XMemPhysical, new UnsupportedDestinationSink(log)), log);
        RecipeAllocator = new RecipeAllocator();
        Input = new GameInputService(new XInputGamepads(), new DesktopInput(), InputConfigFile.Defaults(), environment.Time, Stubs, CodeHeap, log);

        ShimContext.Activate(this);
    }

    /// <summary>The allocator the export-call recipes hand to libHttpClient.</summary>
    public RecipeAllocator RecipeAllocator { get; }

    public LoaderLog Log { get; }

    public LoaderCounters Counters { get; }

    public CodeHeap CodeHeap { get; }

    public LastErrorBridge LastError { get; }

    public ShimTable Shims { get; }

    public UnresolvedStubs Stubs { get; }

    public ImageTable Images { get; }

    public VirtualModules VirtualModules { get; }

    public ImageLoader Loader { get; }

    /// <summary>The ring of recent calls through shims and unresolved stubs, when enabled.</summary>
    public CallTrace Trace { get; }

    /// <summary>The emulation of the Zen 2 instructions an Intel CPU lacks, run by the fault reporter's handler.</summary>
    public AmdInstructionHandler Cpu { get; }

    /// <summary>The vectored exception handler that reports faults in mapped code.</summary>
    public FaultReporter Faults { get; }

    /// <summary>The code patches applied to images as they are mapped.</summary>
    public ImagePatcher Patcher { get; }

    /// <summary>The mod folders the file shims consult.</summary>
    public FileOverlay Overlay { get; }

    public RuntimeLibraryRouter Router { get; }

    public ProcessIdentity Identity { get; }

    public ModuleIdentity ModuleIdentity { get; }

    public XMemAllocator XMem { get; }

    /// <summary>The xmem.dll virtual and physical page functions.</summary>
    public XMemPhysicalMemory XMemPhysical { get; }

    /// <summary>xmem.dll XMemGetWorkingSetStatistics.</summary>
    public XMemWorkingSet XMemWorkingSet { get; }

    /// <summary>The acphal.dll APU heap.</summary>
    public ApuHeap Apu { get; }

    /// <summary>What the audio hardware shims did, for the thread sampler.</summary>
    public AudioCounters Audio { get; }

    /// <summary>The acphal.dll SHAPE contexts and IAcpHal object.</summary>
    public AcpHalDevice AcpHal { get; }

    /// <summary>xapu.dll: the Opus decode queue.</summary>
    public XApuDevice XApu { get; }

    /// <summary>chad.dll: the audio DSP streams.</summary>
    public XDspDevice XDsp { get; }

    /// <summary>The DirectStorage factory XCRDAPI.dll hands out.</summary>
    public DStorageFactory Storage { get; }

    /// <summary>The texture address computers xg_xs.dll hands out.</summary>
    public XgComputers Xg { get; }

    public D3D12Forwarder D3D12 { get; }

    /// <summary>D3D12XboxCreateDevice: the D3D12.X device on a desktop device.</summary>
    public XboxDeviceFactory D3D12X { get; }

    /// <summary>The console FidelityFX loader's entry points, forwarded to a PC FidelityFX DLL when one is given.</summary>
    public FidelityFxBridge FidelityFx { get; }

    /// <summary>XboxBvhBuilder.dll's exports: desktop sizes by default; console sizes and CPU rehydration with --raytracing.</summary>
    public BvhBuilderService BvhBuilder { get; }

    /// <summary>The record of blocked outbound network attempts (WinHTTP, Winsock, libHttpClient).</summary>
    public NetworkSeal Network { get; }

    public GdkServices Gdk { get; }

    /// <summary>GameInputCreate and the gamepad, keyboard and mouse sources behind it.</summary>
    public GameInputService Input { get; }

    public void Dispose()
    {
        Identity.RestorePeb();
        Gdk.Dispose();
        Input.Dispose();
        Storage.Dispose();
        Xg.Dispose();

        XDsp.Dispose();
        XApu.Dispose();
        AcpHal.Dispose();
        Apu.Dispose();
        LastError.Dispose();

        D3D12X.Dispose();
        XMemPhysical.Dispose();
    }

    private GdkServices CreateGdk(GdkOptions options, TimeProvider time)
    {
        var config = new GdkConfig(options, Loader, Log);
        var boundary = new NativeBoundary(Log);
        var taskQueues = new TaskQueueService(boundary);
        var async = new AsyncService(taskQueues, boundary);
        var users = new UserService();
        var saves = new GameSaveService(config, new SaveStore(config, boundary, time), users, async, boundary);

        var packages = new PackageService(config, taskQueues);
        var system = new SystemService(config, boundary);
        var store = new StoreService(config, packages);
        var runtime = new GameRuntimeService(config, Shims, Stubs, CodeHeap, Trace, Log);

        var registrations = new EventRegistrations();
        var connectivity = new ConnectivityService(options.Network, registrations, taskQueues, boundary);
        var platform = new GdkPlatformServices(registrations, new DisplayService(boundary), new AccessibilityService(), new LauncherService(boundary), connectivity);

        return new GdkServices(config, boundary, taskQueues, async, users, saves, packages, system, store, runtime, platform);
    }
}
