// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Text.RegularExpressions;
using FiveStar.Shaders.Analysis;
using FiveStar.Shaders.Translation.Stages;

namespace FiveStar.Shaders.Translation;

/// <summary>
/// Translates a native RDNA program into HLSL (shader model 6.7). One HLSL invocation is one GPU lane: registers are
/// uint variables, EXEC is a per-lane bool, control flow is a wave-uniform dispatch over basic blocks (or, as an option,
/// structured if/else and loops) and resources are bindless heap indices. docs/shader-translation.md describes the model.
/// </summary>
public static partial class HlslTranslator
{
    private const int Wave32 = 32;
    private const int Wave64 = 64;

    /// <summary>The narrowest hardware wave: a group of more threads runs as several.</summary>
    private const int HardwareWave32 = 32;
    private const int DefaultComputeThreads = 64;
    private const int GroupAxes = 3;

    /// <summary>
    /// Translates a program in the dispatch form without wave-size pins, with wave-size emulation; throws
    /// <see cref="UnsupportedProgramException"/> for what it cannot handle.
    /// </summary>
    public static TranslationResult Translate(ShaderProgram program)
    {
        return Translate(program, new TranslationOptions(StructuredControlFlow: false, PinWaveSize: false, EmulateWaveSize: true));
    }

    /// <summary>Translates a program; throws <see cref="UnsupportedProgramException"/> for what it cannot handle.</summary>
    public static TranslationResult Translate(ShaderProgram program, TranslationOptions options)
    {
        string hlsl;

        if (program.Stage == ShaderStage.Geometry)
        {
            hlsl = GeometryProgramBuilder.Build(program, options);
        }
        else
        {
            hlsl = ProgramTranslator.Translate(program, program.Instructions, program.Stage, copyShader: false, options);
        }

        if (options.PinWaveSize && program.Stage == ShaderStage.Compute)
        {
            hlsl = PinWaveSize(hlsl, program.Config.WaveSize, GroupThreads(program));
        }

        return new TranslationResult(program.Stage, hlsl);
    }

    /// <summary>
    /// The variant of a translated wave64 compute program (typically one pinned to <c>[WaveSize(64)]</c>, which it drops)
    /// for GPUs without 64-lane waves: when its thread
    /// group fits in one console wave, the group runs as that wave and the wave operations exchange through groupshared
    /// memory (VW_GROUP_WAVE in rdna_prelude.hlsli). Throws <see cref="UnsupportedProgramException"/> with the reason when
    /// that is not possible: the group holds several console waves, which can branch apart, or a wave operation sits
    /// under a per-lane condition, where not every thread would reach its barrier.
    /// </summary>
    public static string GroupWaveVariant(string hlsl)
    {
        const string Pin = "[WaveSize(64)]\n";

        var threads = NumThreadsPattern().Match(hlsl);

        if (!threads.Success)
        {
            throw new UnsupportedProgramException("no numthreads");
        }

        var group = Enumerable.Range(1, GroupAxes).Aggregate(1, (product, index) => product * int.Parse(threads.Groups[index].Value, System.Globalization.CultureInfo.InvariantCulture));

        if (group > Wave64)
        {
            throw new UnsupportedProgramException($"group of {group} threads holds several waves");
        }

        var line = ConditionalWaveOperation(hlsl);

        if (line is not null)
        {
            throw new UnsupportedProgramException($"wave operation under a per-lane condition: {line}");
        }

        var defines = $"#define VW_GROUP_WAVE 1\n#define VW_WAVE {Wave64}\n#define VW_GROUP_SIZE {group}u\n";
        var body = hlsl.Replace(Pin, string.Empty, StringComparison.Ordinal);

        // More than one 32-lane hardware wave: LDS a lane reads may come from the other one (Waves.GroupLdsBarriers).
        if (group > HardwareWave32)
        {
            body = Waves.GroupLdsBarriers.Insert(body);
        }

        return defines + body;
    }

    /// <summary>
    /// The superstep form of a mesh or compute program for hardware waves narrower than its console wave: every wave
    /// operation is a step of the whole thread group through groupshared memory (rdna_superstep.hlsli), so the program
    /// gives console results on any hardware wave, also when its group holds several console waves that branch apart.
    /// Throws <see cref="UnsupportedProgramException"/> when a statement cannot be cut into steps.
    /// </summary>
    public static string SuperstepVariant(ShaderProgram program, TranslationOptions options)
    {
        if (program.Stage is not (ShaderStage.Mesh or ShaderStage.Compute))
        {
            throw new UnsupportedProgramException("superstep: not a mesh or compute program");
        }

        return ProgramTranslator.TranslateSupersteps(program, options);
    }

    /// <summary>
    /// Adds [WaveSize(n)] (compute only in SM 6.7) when the program reads other lanes by lane number. A wave64 program
    /// whose group has at most 32 threads never has lanes 32..63, so it gets the wave32 pin every GPU offers. A wave32
    /// program with any wave operation gets it too: a wider hardware wave holds two console waves, whose masks hold 32
    /// lanes, so lanes 32..63 read their bit from the half the console does not have (a loop on such a mask never ends).
    /// </summary>
    private static string PinWaveSize(string hlsl, int? waveSize, int groupThreads)
    {
        const string EntryAttribute = "[numthreads(";

        var entry = hlsl.IndexOf(EntryAttribute, StringComparison.Ordinal);

        if (waveSize is null || entry < 0)
        {
            return hlsl;
        }

        var pin = waveSize.Value;

        if (pin == Wave64 && groupThreads <= Wave32)
        {
            pin = Wave32;
        }

        var dependence = WaveUsageAnalyzer.Dependence(hlsl);

        if (dependence != WaveDependence.LaneIndexed && (dependence == WaveDependence.Agnostic || pin != Wave32))
        {
            return hlsl;
        }

        return hlsl.Insert(entry, $"[WaveSize({pin})]\n");
    }

    private static int GroupThreads(ShaderProgram program)
    {
        var threads = program.Config.Compute?.NumThreads;

        if (threads is null)
        {
            return DefaultComputeThreads;
        }

        return threads.Value.X * threads.Value.Y * threads.Value.Z;
    }

    /// <summary>The first line that calls a group-wave operation where only some threads may get to it, else null.</summary>
    private static string? ConditionalWaveOperation(string hlsl)
    {
        foreach (var line in hlsl.Split('\n'))
        {
            foreach (Match call in GroupWaveCallPattern().Matches(line))
            {
                if (PerLaneConditionPattern().Matches(line).Any(condition => condition.Index < call.Index))
                {
                    return line.Trim();
                }
            }
        }

        return null;
    }

    [GeneratedRegex(@"\[numthreads\((\d+), (\d+), (\d+)\)\]")]
    private static partial Regex NumThreadsPattern();

    [GeneratedRegex(@"\b(vw_read|vw_ballot|vw_any|vw_count|readfirstlane)\(")]
    private static partial Regex GroupWaveCallPattern();

    // Before the call: a short-circuit operator (HLSL 2021) or an if on a lane value (wave-uniform conditions are scc or
    // the wave operations themselves).
    [GeneratedRegex(@"&&|\|\||\?|\bif \((?!scc\b|!scc\b|vw_any\(|!vw_any\()")]
    private static partial Regex PerLaneConditionPattern();
}
