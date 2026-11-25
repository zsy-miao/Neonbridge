// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Graphics;
using FiveStar.Graphics.Programs;

namespace FiveStar.Cli.Shader;

/// <summary>
/// The pipelines of a translate run on one adapter: the form of every compiled program the adapter runs
/// (<see cref="VariantSelector"/>): the native DXIL, the wave32 variant of a pinned compute program (kind
/// <c>cs_w32</c>), the superstep form of a mesh program (<c>ms_w32</c>) or the mesh fallback as two pipelines
/// (<c>msfb_cs</c>, <c>msfb_vs</c>). Programs are taken in
/// results.json order, filtered by <c>--stages</c> and then thinned by <c>--step</c> and <c>--limit</c>.
/// </summary>
internal static class PsoTargets
{
    private const string MeshFallbackCompute = "msfb_cs";
    private const string MeshFallbackVertex = "msfb_vs";
    private const string GroupWaveCompute = "cs_w32";
    private const string SuperstepMesh = "ms_w32";

    public static List<PsoTarget> Read(string folder, CommandArguments arguments, AdapterInfo adapter)
    {
        var stages = Stages(arguments.Option("stages"));
        var programs = ManifestBuilder.Load(folder).Programs
            .Where(program => stages is null || stages.Contains(program.Stage))
            .ToList();
        var step = arguments.IntegerOption("step") ?? 1;
        programs = programs.Where((_, index) => index % step == 0).ToList();
        var limit = arguments.IntegerOption("limit");

        if (limit is not null)
        {
            programs = programs.Take(limit.Value).ToList();
        }

        return programs.SelectMany(program => Targets(folder, VariantSelector.Choose(adapter, program))).ToList();
    }

    private static IEnumerable<PsoTarget> Targets(string folder, ProgramChoice choice)
    {
        var name = choice.Program.Name;

        if (choice.Files.Count == 0)
        {
            yield return new PsoTarget(name, choice.Program.Stage, Path.Combine(folder, name + ".dxil"));

            yield break;
        }

        if (choice.Files.Count > 1)
        {
            yield return new PsoTarget(name, MeshFallbackCompute, Path.Combine(folder, choice.Files[0]));

            yield return new PsoTarget(name, MeshFallbackVertex, Path.Combine(folder, choice.Files[1]));

            yield break;
        }

        var kind = choice.Program.Stage;

        if (choice.Outcome == ChoiceOutcome.Variant && kind == "ms")
        {
            kind = SuperstepMesh;
        }
        else if (choice.Outcome == ChoiceOutcome.Variant)
        {
            kind = GroupWaveCompute;
        }

        yield return new PsoTarget(name, kind, Path.Combine(folder, choice.Files[0]));
    }

    private static HashSet<string>? Stages(string? list)
    {
        if (list is null)
        {
            return null;
        }

        return list.Split(',', StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries).ToHashSet(StringComparer.Ordinal);
    }
}
