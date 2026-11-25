// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Collections.Concurrent;
using System.Globalization;
using FiveStar.Shaders.Compilation;
using FiveStar.Shaders.Disassembly;

namespace FiveStar.Cli.Shader;

/// <summary>
/// <c>fivestar shader decode-check</c>: disassembles every program of a folder with llvm-mc/llvm-objdump and with the
/// in-process decoder and reports where the listings differ. Exit code 0 when they agree everywhere.
/// </summary>
internal static class DecodeCheckCommand
{
    private const int Success = 0;
    private const int Failure = 1;
    private const int OnePath = 1;
    private const int MismatchesShown = 20;
    private const int SizeWidth = 2;

    public static int Run(IReadOnlyList<string> args)
    {
        var arguments = new CommandArguments(args, new HashSet<string>());

        if (arguments.Positional.Count != OnePath || !Directory.Exists(arguments.Positional[0]))
        {
            throw new ArgumentException("decode-check <fssm-folder> [--jobs N] [--limit N] [--step K] [--llvm <folder>]");
        }

        var programs = TranslateCommand.Programs(arguments);
        var check = new DecoderCheck(new LlvmDisassembler(ShaderTools.Llvm(arguments)), new RdnaDecoder());
        var results = new ConcurrentBag<DecodeCheckResult>();

        var mismatches = 0;
        var done = 0;
        var progress = new ConsoleProgress(() => string.Create(CultureInfo.InvariantCulture, $"mismatched programs {Volatile.Read(ref mismatches)}"));
        var parallel = new ParallelOptions { MaxDegreeOfParallelism = arguments.IntegerOption("jobs") ?? Environment.ProcessorCount };

        Parallel.ForEach(programs, parallel, program =>
        {
            var result = check.Check(program);
            results.Add(result);

            if (result.Mismatches.Count > 0)
            {
                Interlocked.Increment(ref mismatches);
            }

            progress.Report(new CorpusProgress(Interlocked.Increment(ref done), programs.Count));
        });

        return Report(results.ToList(), programs.Count);
    }

    private static int Report(List<DecodeCheckResult> results, int programs)
    {
        var mismatches = results.SelectMany(result => result.Mismatches).OrderBy(mismatch => mismatch.Program, StringComparer.Ordinal).ToList();
        var blocks = results.Sum(result => result.Blocks);
        var instructions = results.Sum(result => (long)result.Instructions);
        var programsWithMismatch = mismatches.Select(mismatch => mismatch.Program).Distinct(StringComparer.Ordinal).Count();
        Console.WriteLine(string.Create(
            CultureInfo.InvariantCulture,
            $"programs {programs}, code blocks {blocks}, instructions {instructions}, mismatched programs {programsWithMismatch}"));

        foreach (var mismatch in mismatches.Take(MismatchesShown))
        {
            Console.WriteLine($"  {mismatch.Program} ({mismatch.Block})");
            Console.WriteLine($"    llvm:   {Describe(mismatch.Expected)}");
            Console.WriteLine($"    native: {Describe(mismatch.Actual)}");
        }

        if (mismatches.Count > 0)
        {
            return Failure;
        }

        return Success;
    }

    private static string Describe(Instruction? instruction)
    {
        if (instruction is null)
        {
            return "(end of listing)";
        }

        return string.Create(CultureInfo.InvariantCulture, $"{instruction.Address:x6} {instruction.Size.ToString(CultureInfo.InvariantCulture).PadLeft(SizeWidth)}  {instruction.Text}");
    }
}
