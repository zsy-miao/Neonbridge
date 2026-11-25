// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Cli.Graphics;
using System.Globalization;
using FiveStar.Graphics;
using FiveStar.Graphics.Programs;

namespace FiveStar.Cli.Shader;

/// <summary>
/// <c>fivestar shader plan</c>: which form of every compiled program each adapter runs (<see cref="VariantSelector"/>),
/// counted as ok, variant, fallback and incorrect, with the reasons of the incorrect ones.
/// </summary>
internal static class PlanCommand
{
    private const int Success = 0;
    private const int ReasonsShown = 8;
    private const int CountWidth = 7;
    private const string AllProfiles = "all";

    private static readonly (ChoiceOutcome Outcome, string Label)[] Columns =
    [
        (ChoiceOutcome.Native, "ok"), (ChoiceOutcome.Variant, "variant"), (ChoiceOutcome.Fallback, "fallback"),
        (ChoiceOutcome.Incorrect, "incorrect"),
    ];

    public static int Run(IReadOnlyList<string> args)
    {
        var arguments = new CommandArguments(args, new HashSet<string>());

        if (arguments.Positional.Count != 1)
        {
            throw new ArgumentException("plan <translate-out> [--adapter <index|name|warp>] [--profile <name,...|all>]");
        }

        var manifest = ManifestBuilder.Load(arguments.Positional[0]);
        Console.WriteLine($"{manifest.Programs.Count} compiled programs");

        foreach (var adapter in Adapters(arguments.Option("adapter"), arguments.Option("profile")))
        {
            Print(adapter, VariantSelector.Plan(adapter, manifest.Programs));
        }

        return Success;
    }

    /// <summary>The chosen adapter and profiles; every adapter with a D3D12 device when neither is chosen.</summary>
    private static IReadOnlyList<AdapterInfo> Adapters(string? choice, string? profiles)
    {
        var adapters = new List<AdapterInfo>();

        if (choice is not null)
        {
            adapters.Add(PsoCommand.SelectAdapter(choice));
        }

        if (profiles is not null)
        {
            adapters.AddRange(Profiles(profiles));
        }

        if (adapters.Count > 0)
        {
            return adapters;
        }

        return MeasuredAdapters.List().Where(adapter => adapter.ShaderModel is not null).ToList();
    }

    private static IEnumerable<AdapterInfo> Profiles(string list)
    {
        if (list.Equals(AllProfiles, StringComparison.OrdinalIgnoreCase))
        {
            return AdapterProfiles.All.Values;
        }

        return list.Split(',', StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries)
            .Select(name => AdapterProfiles.Find(name) ?? throw new ArgumentException($"no profile '{name}' (profiles: {string.Join(", ", AdapterProfiles.All.Keys)})"));
    }

    private static void Print(AdapterInfo adapter, IReadOnlyList<ProgramChoice> choices)
    {
        var counts = Columns.Select(column => $"{column.Label} {Count(choices.Count(choice => choice.Outcome == column.Outcome))}");
        Console.WriteLine();
        Console.WriteLine($"{Label(adapter)}  {adapter.Name}: wave lanes {adapter.WaveLaneCountMin}-{adapter.WaveLaneCountMax}, mesh {adapter.MeshShaderTier}");
        Console.WriteLine("   " + string.Join(", ", counts));

        // The fallback runs as a compute and a vertex shader; it can also be incorrect for its floor.
        var fallback = choices.Where(choice => choice.Files.Count > 1).ToList();

        if (fallback.Count > 0)
        {
            var incorrect = fallback.Count(choice => choice.Outcome == ChoiceOutcome.Incorrect);
            Console.WriteLine($"   mesh programs through the fallback: {Count(fallback.Count)}, {Count(incorrect)} of them incorrect");
        }

        var groups = choices.Where(choice => choice.Outcome != ChoiceOutcome.Native)
            .GroupBy(choice => (choice.Outcome, choice.Program.Stage, choice.Reason))
            .OrderByDescending(group => group.Count())
            .Take(ReasonsShown);

        foreach (var group in groups)
        {
            var label = Columns.First(column => column.Outcome == group.Key.Outcome).Label;
            Console.WriteLine($"{Count(group.Count()).PadLeft(CountWidth)} {label} {group.Key.Stage}: {group.Key.Reason}");
        }
    }

    private static string Label(AdapterInfo adapter)
    {
        if (adapter.Index < 0)
        {
            return "profile";
        }

        return Count(adapter.Index);
    }

    private static string Count(int value)
    {
        return value.ToString(CultureInfo.InvariantCulture);
    }
}
