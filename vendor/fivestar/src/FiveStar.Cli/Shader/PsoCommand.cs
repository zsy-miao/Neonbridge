// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Cli.Graphics;
using System.Diagnostics;
using System.Globalization;
using System.Text;
using FiveStar.Graphics;
using FiveStar.Graphics.Pipelines;
using FiveStar.Shaders.Compilation;

namespace FiveStar.Cli.Shader;

/// <summary>
/// <c>fivestar shader pso</c>: creates a D3D12 pipeline state for every compiled program of a translate run on one
/// adapter and logs whether the driver accepts it; <c>--list-adapters</c> prints the adapters and their wave sizes.
/// </summary>
internal static class PsoCommand
{
    private const int Success = 0;
    private const int Failure = 1;
    private const string ListFlag = "list-adapters";
    private const string DebugFlag = "debug-layer";
    private const string RecheckFlag = "recheck";
    private const string PassThroughFolder = "pso-vs";
    private const string WarpAlias = "warp";
    private const int DefaultTimeoutSeconds = 600;
    private const int DefaultMemoryGigabytes = 16;
    private const long BytesPerGigabyte = 1L << 30;

    public static int Run(IReadOnlyList<string> args)
    {
        var arguments = new CommandArguments(args, new HashSet<string> { ListFlag, DebugFlag, RecheckFlag });

        if (arguments.Flag(ListFlag))
        {
            return ListAdapters();
        }

        if (arguments.Positional.Count != 1)
        {
            throw new ArgumentException("pso <translate-out> [--adapter <index|name>] [--stages ...] [--limit N] [--step K] [--jobs N] [--timeout S] [--max-memory GB] [--debug-layer] [--recheck]");
        }

        var folder = arguments.Positional[0];
        var adapter = SelectAdapter(arguments.Option("adapter"));
        var debug = arguments.Flag(DebugFlag);
        var jobs = 1;

        if (!debug)
        {
            jobs = arguments.IntegerOption("jobs") ?? 1;
        }

        var targets = PsoTargets.Read(folder, arguments, adapter);
        using var log = new PsoLog(Path.Combine(folder, $"pso-{Slug(adapter.Name)}.jsonl"));
        var done = log.Load();
        var pending = targets.Where(target => !IsDone(done, target, arguments.Flag(RecheckFlag))).ToList();
        Console.WriteLine($"{adapter.Name}: {targets.Count} pipelines, {pending.Count} to create, log {log.ResultsPath}");

        var clock = Stopwatch.StartNew();
        var complete = Sweep(adapter, debug, jobs, folder, arguments, log, pending);
        Console.WriteLine($"created {pending.Count} in {clock.Elapsed:hh\\:mm\\:ss}");

        var selected = targets.Select(target => target.Key).ToHashSet(StringComparer.Ordinal);
        PsoSummary.Print(log.Load().Values.Where(record => selected.Contains(record.Key)).ToList());

        if (!complete)
        {
            Console.Error.WriteLine("device removed: the sweep stopped; run again to resume");

            return Failure;
        }

        return Success;
    }

    /// <summary>The measured mesh-shader wave (MeshWaveProbe), or nothing when it was not measured.</summary>
    private static string MeshWave(AdapterInfo adapter)
    {
        if (adapter.MeshWaveLanes is null)
        {
            return string.Empty;
        }

        return $" ({adapter.MeshWaveLanes}-lane mesh waves)";
    }

    /// <summary>Creates the pending pipelines on the adapter within the limits of the arguments; false when the device was removed.</summary>
    internal static bool Sweep(AdapterInfo adapter, bool debug, int jobs, string folder, CommandArguments arguments, PsoLog log, List<PsoTarget> pending)
    {
        using var device = new GpuDevice(adapter.Index, debug);
        using var root = BindlessRootSignature.Create(device.Device);
        var cache = new PassThroughCache(ShaderTools.Compiler(arguments, retry: false), Path.Combine(folder, PassThroughFolder));
        var pipelines = new ProgramPipelines(new PipelineBuilder(device.Device, root), cache);

        if (!device.DisableDriverShaderCache())
        {
            Console.Error.WriteLine("the driver shader cache stays on (needs Windows developer mode); it grows with every pipeline");
        }

        var timeout = TimeSpan.FromSeconds(arguments.IntegerOption("timeout") ?? DefaultTimeoutSeconds);
        var memory = (arguments.IntegerOption("max-memory") ?? DefaultMemoryGigabytes) * BytesPerGigabyte;
        var sweep = new PsoSweep(device, pipelines, log, new PsoLimits(jobs, timeout, memory));
        var complete = sweep.Run(pending);
        var peak = ((double)sweep.PeakBytes / BytesPerGigabyte).ToString("F1", CultureInfo.InvariantCulture);
        Console.WriteLine($"peak private memory {peak} GB");

        return complete;
    }

    private static bool IsDone(Dictionary<string, PsoRecord> done, PsoTarget target, bool recheck)
    {
        if (!done.TryGetValue(target.Key, out var record))
        {
            return false;
        }

        return !recheck || record.Result == PsoRecord.Accepted;
    }

    private static int ListAdapters()
    {
        foreach (var adapter in MeasuredAdapters.List())
        {
            var kind = string.Empty;

            if (adapter.Software)
            {
                kind = " (software)";
            }

            if (adapter.ShaderModel is null)
            {
                Console.WriteLine($"{adapter.Index}  {adapter.Name}{kind}: no D3D12 device");
                continue;
            }

            Console.WriteLine($"{adapter.Index}  {adapter.Name}{kind}: SM {adapter.ShaderModel}, wave lanes {adapter.WaveLaneCountMin}-{adapter.WaveLaneCountMax}, mesh {adapter.MeshShaderTier}{MeshWave(adapter)}");
        }

        return Success;
    }

    /// <summary>
    /// The adapter by index, by a name substring or <c>warp</c> (the software adapter, named Microsoft Basic Render
    /// Driver); by default the first hardware adapter with a D3D12 device.
    /// </summary>
    internal static AdapterInfo SelectAdapter(string? choice)
    {
        var adapters = MeasuredAdapters.List().Where(adapter => adapter.ShaderModel is not null).ToList();
        AdapterInfo? selected;

        if (choice is null)
        {
            selected = adapters.FirstOrDefault(adapter => !adapter.Software);
        }
        else if (choice.Equals(WarpAlias, StringComparison.OrdinalIgnoreCase))
        {
            selected = adapters.FirstOrDefault(adapter => adapter.Software);
        }
        else if (int.TryParse(choice, out var index))
        {
            selected = adapters.FirstOrDefault(adapter => adapter.Index == index);
        }
        else
        {
            selected = adapters.FirstOrDefault(adapter => adapter.Name.Contains(choice, StringComparison.OrdinalIgnoreCase));
        }

        return selected ?? throw new ArgumentException($"no D3D12 adapter matches '{choice}' (see --list-adapters)");
    }

    /// <summary>The adapter name in lower case with runs of other characters as one dash: nvidia-geforce-rtx-4090.</summary>
    internal static string Slug(string name)
    {
        var slug = new StringBuilder();

        foreach (var character in name.ToLowerInvariant())
        {
            if (char.IsAsciiLetterOrDigit(character))
            {
                slug.Append(character);
            }
            else if (slug.Length > 0 && slug[^1] != '-')
            {
                slug.Append('-');
            }
        }

        return slug.ToString().TrimEnd('-');
    }
}
