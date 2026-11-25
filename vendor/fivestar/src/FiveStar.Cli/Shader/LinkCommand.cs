// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Collections.Concurrent;
using FiveStar.Shaders.Cache;

namespace FiveStar.Cli.Shader;

/// <summary>
/// <c>fivestar shader link</c>: takes the VS+PS and MS+PS pairs of the console pipeline cache whose programs a
/// translate run compiled, links each producer to its pixel shader (<see cref="LinkedProducers"/>) and creates the
/// pipeline on one adapter, logged as <c>fivestar shader pso</c> logs (<c>&lt;out&gt;/link-&lt;adapter&gt;.jsonl</c>).
/// </summary>
internal static class LinkCommand
{
    private const int Success = 0;
    private const int Failure = 1;
    private const int TwoPaths = 2;
    private const string DebugFlag = "debug-layer";
    private const string RecheckFlag = "recheck";
    private const string UnlinkedFlag = "unlinked";
    private const string ProgramFolderVariable = "FIVESTAR_SHADER_DIR";

    public static int Run(IReadOnlyList<string> args)
    {
        var arguments = new CommandArguments(args, new HashSet<string> { DebugFlag, RecheckFlag, UnlinkedFlag });

        if (arguments.Positional.Count != TwoPaths)
        {
            throw new ArgumentException(
                "link <translate-out> <shaders.rpf | cache blob> [--programs <fssm folder>] [--adapter <index|name>] [--limit N] [--step K] [--jobs N] [--debug-layer] [--unlinked] [--recheck]");
        }

        var folder = arguments.Positional[0];
        var adapter = PsoCommand.SelectAdapter(arguments.Option("adapter"));
        var pairs = Pairs(folder, arguments);

        var unlinked = arguments.Flag(UnlinkedFlag);
        var producers = new LinkedProducers(folder, ShaderTools.Compiler(arguments, retry: false), adapter, unlinked);
        var prefix = unlinked ? "unlinked" : "link";
        using var log = new PsoLog(Path.Combine(folder, $"{prefix}-{PsoCommand.Slug(adapter.Name)}.jsonl"));

        // By pair name: a pair whose linked mesh shader ran into the output limit was logged with the fallback's kind.
        var done = Latest(log);
        var pending = pairs.Where(pair => !IsDone(done, pair, arguments.Flag(RecheckFlag))).ToList();
        Console.WriteLine($"{adapter.Name}: {pairs.Count} pairs, {pending.Count} to create, log {log.ResultsPath}");

        // Compiles run in parallel even with the debug layer, which needs one pipeline at a time.
        var targets = Prepare(producers, pending, arguments.IntegerOption("jobs") ?? 1, log);
        Console.WriteLine($"producers linked {producers.Linked}, already matching {producers.Unchanged}, setup failures {pending.Count - targets.Count}");

        var complete = PsoCommand.Sweep(adapter, arguments.Flag(DebugFlag), Jobs(arguments), folder, arguments, log, targets);
        var selected = pairs.Select(pair => pair.Name).ToHashSet(StringComparer.Ordinal);
        PsoSummary.Print(Latest(log).Values.Where(record => selected.Contains(record.Name)).ToList());

        if (!complete)
        {
            Console.Error.WriteLine("device removed: the run stopped; run again to resume");

            return Failure;
        }

        return Success;
    }

    private static List<LinkPair> Pairs(string folder, CommandArguments arguments)
    {
        var programFolder = arguments.Option("programs") ?? Environment.GetEnvironmentVariable(ProgramFolderVariable)
            ?? throw new ArgumentException("--programs <fssm folder> (or FIVESTAR_SHADER_DIR) is needed to match cache shaders to programs");
        var compiled = ManifestBuilder.Load(folder).Programs.ToDictionary(program => program.Name, StringComparer.Ordinal);
        var cache = PipelineCacheSource.Load(arguments.Positional[1]);
        var pairs = LinkPairs.Find(cache, CacheProgramMap.Build(cache, ProgramIndex.Load(programFolder)), compiled);

        var step = arguments.IntegerOption("step") ?? 1;
        pairs = pairs.Where((_, index) => index % step == 0).ToList();
        var limit = arguments.IntegerOption("limit");

        if (limit is not null)
        {
            pairs = pairs.Take(limit.Value).ToList();
        }

        return pairs;
    }

    /// <summary>One record per pair, the accepted one when there is one.</summary>
    private static Dictionary<string, PsoRecord> Latest(PsoLog log)
    {
        return log.Load().Values
            .GroupBy(record => record.Name, StringComparer.Ordinal)
            .ToDictionary(group => group.Key, group => group.OrderBy(record => record.Result == PsoRecord.Accepted).Last(), StringComparer.Ordinal);
    }

    private static bool IsDone(Dictionary<string, PsoRecord> done, LinkPair pair, bool recheck)
    {
        if (!done.TryGetValue(pair.Name, out var record))
        {
            return false;
        }

        return !recheck || record.Result == PsoRecord.Accepted;
    }

    private static int Jobs(CommandArguments arguments)
    {
        if (arguments.Flag(DebugFlag))
        {
            return 1;
        }

        return arguments.IntegerOption("jobs") ?? 1;
    }

    /// <summary>Links and compiles the producers, N at once; a pair whose link fails is logged as setup-fail.</summary>
    private static List<PsoTarget> Prepare(LinkedProducers producers, List<LinkPair> pairs, int jobs, PsoLog log)
    {
        var targets = new ConcurrentBag<(int Index, PsoTarget Target)>();
        var options = new ParallelOptions { MaxDegreeOfParallelism = jobs };

        Parallel.For(0, pairs.Count, options, index =>
        {
            var pair = pairs[index];

            try
            {
                targets.Add((index, producers.Prepare(pair)));
            }
            catch (PipelineSetupException exception)
            {
                log.Append(new PsoRecord(pair.Name, producers.Kind(pair), PsoRecord.SetupFailed, null, exception.Message, null, 0));
            }
        });

        return targets.OrderBy(entry => entry.Index).Select(entry => entry.Target).ToList();
    }
}
