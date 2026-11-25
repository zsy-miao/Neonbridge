// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Diagnostics;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Loading;
using FiveStar.Loader.Native;
using FiveStar.Loader.Shims;
using FiveStar.Loader.Shims.D3D12X;
using FiveStar.Loader.Shims.D3D12X.DebugLayer;
using FiveStar.Loader.Shims.D3D12X.Pipelines;
using FiveStar.Shaders.Cache;

namespace FiveStar.Loader.Host;

/// <summary>
/// The <c>--warm-pipelines</c> mode: no game runs. On the default adapter, as the game's device would, every pipeline
/// of the console pipeline cache is built through the same factory as slots 57 and 67 and stored in the disk cache
/// (<see cref="PipelineWarmup"/>), so the game's first boot loads them instead of waiting for the driver.
/// </summary>
internal static unsafe class WarmPipelinesCommand
{
    /// <summary>The exit code of a run stopped by Ctrl+C; a new run resumes.</summary>
    private const int Stopped = 4;

    /// <summary>The most builds at once by default: each driver compile can take gigabytes.</summary>
    private const int DefaultJobs = 4;

    public static int Run(HostOptions options, LoaderLog log)
    {
        ShimContext.UseLog(log);

        // --min-free-memory and --max-commit end the warm-up as they end a boot, its helper processes with it.
        if (options.MinimumFreeMemory != 0 || options.MaximumCommit != 0)
        {
            new MemoryLimit(options.MinimumFreeMemory, options.MaximumCommit, null, log).Start();
        }

        var folders = HostComposition.CreatePipelineFolders(options);

        if (folders.Programs is null || (folders.Translated is null && folders.OnDemand is null) || folders.Cache is null)
        {
            log.Error("--warm-pipelines needs --shader-dir, --translated-dir or a compiler for programs on demand, and a pipeline cache (not --no-pipeline-cache)");

            return LoaderHost.UsageError;
        }

        var device = new D3D12Forwarder(log);
        var iid = DesktopDevice.Device2Iid;
        nint device2;
        nint root;

        // --d3d12-debug: the runtime's reason for a refused pipeline (E_INVALIDARG) goes to the log.
        using var debugging = new D3D12Debugging(device, new ImageTable(), log);

        if (options.D3D12Debug)
        {
            debugging.Enable(options.D3D12GpuValidation);
        }

        debugging.BeforeDevice();

        if (HResult.Failed(device.CreateDevice(D3D12XLayout.MinimumFeatureLevel, &iid, &device2)) || HResult.Failed(BindlessRoot.Create(device2, &root)))
        {
            log.Error("no D3D12 device or root signature on the default adapter");

            return LoaderHost.Failure;
        }

        debugging.AttachDevice(device2);
        var cache = PipelineLibraryCache.Open(folders.Cache, device2, log);

        if (cache is null)
        {
            return LoaderHost.Failure;
        }

        using (cache)
        {
            cache.StartSaving();

            return Warm(options, folders, device2, root, cache, log);
        }
    }

    private static int Warm(HostOptions options, PipelineFolders folders, nint device2, nint root, PipelineLibraryCache cache, LoaderLog log)
    {
        var warnings = new OnceLog(log);
        var index = ProgramIndex.Load(folders.Programs!);
        var (library, onDemand, forget) = Library(folders, index, device2, warnings, log);

        var builds = new PipelineBuildQueue(1, PipelineBuildQueue.AnyTime);
        var standIns = new StandInPipelines(PipelineSlots.CreationBudget, builds, builds, warnings);
        var factory = new PipelineFactory(device2, root, library, new ProgramSignatures(folders.Programs!), warnings, null, standIns, cache) { OnDemand = onDemand, LinkInline = true };
        var warmup = new PipelineWarmup(factory, cache, folders.Cache!, log);

        // The pipelines earlier boots asked for, in first-use order, then (unless --warm-used-only) the console cache's.
        var used = PipelineJournal.Read(folders.Cache!);
        List<WarmTarget> targets = [.. used];

        if (!options.WarmUsedOnly)
        {
            targets.AddRange(WarmTargets.Read(ConsolePipelineSource.Load(options.WarmSource!), 1));
        }

        targets = Matching(targets.Where((_, position) => position % Math.Max(1, options.WarmStep) == 0).ToList(), options.WarmMatches, index);
        var jobs = options.WarmJobs ?? Math.Clamp(Environment.ProcessorCount / PipelineSlots.CoresPerBuildThread, 1, DefaultJobs);
        log.Info($"warm-up: {targets.Count} pipelines ({used.Count} from {PipelineJournal.FileName} first) on {jobs} threads into {folders.Cache}");

        ConsoleCancelEventHandler stop = (_, args) =>
        {
            args.Cancel = true;
            warmup.Stop();
            log.Info("warm-up stopping: the running builds finish, then the cache is written");
        };
        Console.CancelKeyPress += stop;
        var clock = Stopwatch.StartNew();

        try
        {
            if (!warmup.Run(targets, jobs, forget))
            {
                log.Info($"warm-up stopped after {clock.Elapsed:hh\\:mm\\:ss}: {warmup.Tally.Summary}; run again to resume");

                return Stopped;
            }

            log.Info($"warm-up done in {clock.Elapsed:hh\\:mm\\:ss}: {warmup.Tally.Summary}");

            return LoaderHost.Success;
        }
        finally
        {
            Console.CancelKeyPress -= stop;
        }
    }

    /// <summary>
    /// The translation, through the program cache too when there is a compiler: producers that need linking to their
    /// pixel shader are then linked on the warm-up's threads (<see cref="ProducerLinks"/>). With how to drop read DXIL.
    /// </summary>
    private static (IProgramLibrary Library, OnDemandPrograms? OnDemand, Action Forget) Library(PipelineFolders folders, ProgramIndex index, nint device2, OnceLog warnings, LoaderLog log)
    {
        var onDemand = folders.OnDemand?.Open(index, folders.Programs!, folders.Translated, device2, warnings, log);

        if (onDemand is not null)
        {
            return (onDemand, onDemand, onDemand.Forget);
        }

        var translated = TranslatedProgramLibrary.Open(index, folders.Translated!, device2, warnings, log);

        return (translated, null, translated.Forget);
    }

    /// <summary>The targets with a shader whose extracted program name contains one of <paramref name="matches"/>; all without.</summary>
    private static List<WarmTarget> Matching(List<WarmTarget> targets, IReadOnlyList<string> matches, ProgramIndex index)
    {
        if (matches.Count == 0)
        {
            return targets;
        }

        return targets.Where(target => Shaders(target).Any(shader => index.Find(shader.Bytes).Any(name => matches.Any(match => name.Contains(match, StringComparison.OrdinalIgnoreCase))))).ToList();
    }

    private static IEnumerable<ShaderSource> Shaders(WarmTarget target)
    {
        if (target.Compute is not null)
        {
            return [target.Compute.Shader];
        }

        return target.Graphics!.Shaders;
    }
}
