// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Collections.Concurrent;
using System.Diagnostics;
using System.Globalization;
using FiveStar.Shaders.Dxil;
using FiveStar.Shaders.Dxil.Container;
using FiveStar.Shaders.Dxil.Hlsl;
using FiveStar.Shaders.Dxil.Ir;
using FiveStar.Shaders.Dxil.Lowering;
using FiveStar.Shaders.Dxil.Validation;
using FiveStar.Shaders.Translation;

namespace FiveStar.Cli.Shader;

/// <summary>
/// <c>fivestar shader dxil</c>: compiles translated HLSL with the DXIL backend (no dxc), optionally validates each
/// container with dxil.dll, and reports coverage, failure reasons and compile times. Each program's line goes to
/// <c>&lt;out&gt;/dxil-backend.tsv</c>: name, source characters, milliseconds, DXIL bytes, status, reason.
/// </summary>
internal static class DxilCommand
{
    private const int Success = 0;
    private const int OnePath = 1;
    private const int TopReasons = 25;
    private const int ReasonLength = 160;
    private const string ValidateFlag = "validate";
    private const string PhasesFlag = "phases";
    private const string CompareFlag = "compare";
    private const string NoOptimizeFlag = "no-optimize";
    private const string LogName = "dxil-backend.tsv";

    public static int Run(IReadOnlyList<string> args)
    {
        var arguments = new CommandArguments(args, new HashSet<string> { ValidateFlag, PhasesFlag, CompareFlag, NoOptimizeFlag });

        if (arguments.Positional.Count != OnePath)
        {
            throw new ArgumentException("dxil <hlsl file or folder> [--out <dir>] [--validate] [--compare] [--stages cs,ps,vs] [--limit N] [--step K] [--jobs N] [--repeat N] [--dxc <path>] [--no-optimize]");
        }

        DxilBackend.MemoryLocalsCases = arguments.IntegerOption("memory-locals") ?? 0;
        DxilBackend.Optimize = !arguments.Flag(NoOptimizeFlag);

        var files = Sources(arguments);
        var output = arguments.Option("out");
        using var validator = Validator(arguments);
        var results = new ConcurrentBag<Outcome>();
        var options = new ParallelOptions { MaxDegreeOfParallelism = arguments.IntegerOption("jobs") ?? 1 };

        if (output is not null)
        {
            Directory.CreateDirectory(output);
        }

        var repeat = arguments.IntegerOption("repeat") ?? 1;

        if (arguments.Flag(PhasesFlag))
        {
            Phases(files, repeat, arguments.Option("ir"));

            return Success;
        }

        SignatureCheck? check = null;

        if (arguments.Flag(CompareFlag))
        {
            check = new SignatureCheck(ShaderTools.Compiler(arguments, retry: false), Path.Combine(output ?? Path.GetTempPath(), "dxc"));
        }

        Parallel.ForEach(files, options, file => results.Add(CompileOne(file, output, validator, repeat, check)));
        Report([.. results.OrderBy(result => result.Name, StringComparer.Ordinal)], output);

        return Success;
    }

    private static List<string> Sources(CommandArguments arguments)
    {
        var path = arguments.Positional[0];
        IEnumerable<string> files = [path];

        if (Directory.Exists(path))
        {
            files = Directory.EnumerateFiles(path, "*.hlsl").Order(StringComparer.Ordinal);
        }

        var step = arguments.IntegerOption("step") ?? 1;
        var selected = files.Where((_, index) => index % step == 0);
        var limit = arguments.IntegerOption("limit");

        if (limit is not null)
        {
            selected = selected.Take(limit.Value);
        }

        var stages = (arguments.Option("stages") ?? "cs,ps,vs").Split(',', StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries).ToHashSet(StringComparer.Ordinal);

        return [.. selected.Where(file => stages.Contains(Profile(File.ReadAllText(file))))];
    }

    /// <summary>The profile of a translated program, from its entry point's attributes and semantics.</summary>
    public static string Profile(string hlsl)
    {
        if (hlsl.Contains("outputtopology", StringComparison.Ordinal))
        {
            return "ms";
        }

        if (hlsl.Contains("[numthreads(", StringComparison.Ordinal))
        {
            return "cs";
        }

        if (hlsl.Contains("[maxvertexcount(", StringComparison.Ordinal))
        {
            return "gs";
        }

        if (hlsl.Contains(": SV_VertexID", StringComparison.Ordinal))
        {
            return "vs";
        }

        return "ps";
    }

    private static DxilValidator? Validator(CommandArguments arguments)
    {
        if (!arguments.Flag(ValidateFlag))
        {
            return null;
        }

        var folder = Path.GetDirectoryName(ShaderTools.Dxc(arguments)) ?? string.Empty;

        return DxilValidator.Load(Path.Combine(folder, "dxil.dll"));
    }

    /// <summary>Compiles one program <paramref name="repeat"/> times (the last run is timed, after JIT warm-up).</summary>
    private static Outcome CompileOne(string file, string? output, DxilValidator? validator, int repeat, SignatureCheck? check)
    {
        var name = Path.GetFileNameWithoutExtension(file);
        var hlsl = File.ReadAllText(file);
        var profile = Profile(hlsl);
        var clock = Stopwatch.StartNew();
        byte[] dxil = [];

        try
        {
            for (var run = 0; run < repeat; run++)
            {
                clock.Restart();
                dxil = DxilBackend.Compile(hlsl, profile);
            }
        }
        catch (DxilBackendException exception)
        {
            return new Outcome(name, hlsl.Length, clock.Elapsed.TotalMilliseconds, 0, "fail", exception.Message);
        }

        var milliseconds = clock.Elapsed.TotalMilliseconds;

        if (output is not null)
        {
            File.WriteAllBytes(Path.Combine(output, name + ".dxil"), dxil);
        }

        var errors = validator?.Validate((byte[])dxil.Clone());

        if (errors is not null)
        {
            return new Outcome(name, hlsl.Length, milliseconds, dxil.Length, "invalid", errors.Replace('\n', ' '));
        }

        var difference = check?.Compare(file, hlsl, profile, dxil);

        if (difference is not null)
        {
            return new Outcome(name, hlsl.Length, milliseconds, dxil.Length, "signature", difference);
        }

        return new Outcome(name, hlsl.Length, milliseconds, dxil.Length, "ok", string.Empty);
    }

    /// <summary>Times the backend's phases on each program (the last of <paramref name="repeat"/> runs) and counts what each allocates.</summary>
    private static void Phases(List<string> files, int repeat, string? listing)
    {
        foreach (var file in files)
        {
            var hlsl = File.ReadAllText(file);
            var marks = new (double Milliseconds, long Bytes)[PhaseCount];

            for (var run = 0; run < repeat; run++)
            {
                marks = TimePhases(hlsl, listing, run == repeat - 1);
            }

            var parts = PhaseNames.Select((name, index) =>
            {
                var (milliseconds, bytes) = marks[index];

                if (index > 0)
                {
                    milliseconds -= marks[index - 1].Milliseconds;
                    bytes -= marks[index - 1].Bytes;
                }

                return string.Create(CultureInfo.InvariantCulture, $"{name} {milliseconds:F1} ms {bytes / BytesPerMegabyte} MB");
            });
            Console.WriteLine($"{Path.GetFileName(file)}: {string.Join(", ", parts)}");
        }
    }

    /// <summary>One compile, phase by phase: the elapsed milliseconds and allocated bytes after each; the last run prints the IR size.</summary>
    private static (double Milliseconds, long Bytes)[] TimePhases(string hlsl, string? listing, bool last)
    {
        var marks = new (double Milliseconds, long Bytes)[PhaseCount];
        var allocated = GC.GetAllocatedBytesForCurrentThread();
        var paused = GC.GetTotalPauseDuration();
        var clock = Stopwatch.StartNew();

        var tokens = Preprocessor.Run(hlsl, RuntimeIncludes.Read);
        marks[0] = (clock.Elapsed.TotalMilliseconds, GC.GetAllocatedBytesForCurrentThread() - allocated);
        var tree = Parser.Parse(tokens);
        marks[1] = (clock.Elapsed.TotalMilliseconds, GC.GetAllocatedBytesForCurrentThread() - allocated);

        var (module, main, description) = ShaderLowering.Lower(tree, DxilBackend.KindOf(Profile(hlsl)));
        marks[PhaseLower] = (clock.Elapsed.TotalMilliseconds, GC.GetAllocatedBytesForCurrentThread() - allocated);
        var container = ShaderContainer.Write(module, main, description);
        marks[PhaseWrite] = (clock.Elapsed.TotalMilliseconds, GC.GetAllocatedBytesForCurrentThread() - allocated);

        if (last && listing is not null)
        {
            File.WriteAllText(listing, IrText.Print(main));
        }

        if (last)
        {
            var instructions = main.Blocks.Sum(block => block.Body.Count);
            var pause = (GC.GetTotalPauseDuration() - paused).TotalMilliseconds;
            Console.WriteLine(string.Create(CultureInfo.InvariantCulture, $"blocks {main.Blocks.Count}, instructions {instructions}, phis {main.Blocks.Sum(block => block.Phis.Count)}, bytes {container.Length}, GC pauses {pause:F0} ms"));
        }

        return marks;
    }

    private const int PhaseCount = 4;
    private const long BytesPerMegabyte = 1 << 20;
    private static readonly string[] PhaseNames = ["preprocess", "parse", "lower", "write"];
    private const int Half = 2;
    private const int CountWidth = 5;
    private const double MillisecondsPerSecond = 1000;
    private const int PhaseLower = 2;
    private const int PhaseWrite = 3;

    private static void Report(List<Outcome> results, string? output)
    {
        if (output is not null)
        {
            var lines = results.Select(result => string.Create(
                CultureInfo.InvariantCulture,
                $"{result.Name}\t{result.Characters}\t{result.Milliseconds:F1}\t{result.Bytes}\t{result.Status}\t{result.Reason}"));
            File.WriteAllLines(Path.Combine(output, LogName), lines);
        }

        foreach (var group in results.GroupBy(result => result.Status).OrderBy(group => group.Key, StringComparer.Ordinal))
        {
            var times = group.Select(result => result.Milliseconds).Order().ToList();
            var median = times[times.Count / Half];
            Console.WriteLine(string.Create(CultureInfo.InvariantCulture, $"{group.Key}: {group.Count()} programs, {times.Sum() / MillisecondsPerSecond:F1} s, median {median:F1} ms, max {times[^1]:F0} ms"));
        }

        var reasons = results.Where(result => result.Status != "ok")
            .GroupBy(result => Normalize(result.Reason))
            .OrderByDescending(group => group.Count())
            .Take(TopReasons);

        foreach (var reason in reasons)
        {
            Console.WriteLine($"  {reason.Count().ToString(CultureInfo.InvariantCulture).PadLeft(CountWidth)}  {reason.Key}   e.g. {reason.First().Name}");
        }
    }

    /// <summary>A reason without line numbers and names, so equal causes group.</summary>
    private static string Normalize(string reason)
    {
        var text = System.Text.RegularExpressions.Regex.Replace(reason, @"line \d+: ", string.Empty);
        text = System.Text.RegularExpressions.Regex.Replace(text, @"\d+", "#");

        return text[..Math.Min(text.Length, ReasonLength)];
    }

    private sealed record Outcome(string Name, int Characters, double Milliseconds, int Bytes, string Status, string Reason);
}
