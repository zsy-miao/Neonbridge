// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Graphics;
using FiveStar.Graphics.Programs;

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>
/// Picks the DXIL file of a translated program for the desktop adapter from the translation's <c>variants.json</c>
/// (<see cref="VariantSelector"/>): the wave32 variant of a compute program pinned to a wave size the adapter lacks,
/// the superstep form of a mesh program below its floor, else the native DXIL. A pipeline takes one file per stage,
/// so a mesh fallback (compute plus vertex shader) is not used here and the native mesh shader is tried. A compute
/// program pinned to a wave size the adapter lacks and without a variant has no file: the driver would refuse it.
/// </summary>
internal sealed class ProgramVariantPicker
{
    private const string Compute = "cs";

    private readonly Dictionary<string, ProgramVariants> _programs;
    private readonly AdapterInfo _adapter;
    private readonly OnceLog _warnings;

    public ProgramVariantPicker(ShaderManifest manifest, AdapterInfo adapter, OnceLog warnings)
    {
        _programs = manifest.Programs.ToDictionary(program => program.Name, StringComparer.Ordinal);
        _adapter = adapter;
        _warnings = warnings;
    }

    /// <summary>The file name to load for <paramref name="name"/>, or null when no form loads on the adapter.</summary>
    public string? File(string name, string nativeFile)
    {
        if (!_programs.TryGetValue(name, out var program))
        {
            return nativeFile;
        }

        return Pick(program);
    }

    /// <summary>The file of <paramref name="program"/>'s forms the adapter loads, or null when none loads.</summary>
    public string? Pick(ProgramVariants program)
    {
        var name = program.Name;
        var nativeFile = name + ".dxil";
        var choice = VariantSelector.Choose(_adapter, program);

        switch (choice.Outcome)
        {
            case ChoiceOutcome.Native or ChoiceOutcome.Variant when choice.Files.Count > 0:
                return choice.Files[0];
            case ChoiceOutcome.Native or ChoiceOutcome.Variant:
                return nativeFile;
            case ChoiceOutcome.Fallback:
                Warn(name, $"needs its mesh fallback ({choice.Reason}), which pipelines do not run; trying the native mesh shader");

                return program.Dxil;
            default:
                return Incorrect(program, choice);
        }
    }

    private string? Incorrect(ProgramVariants program, ProgramChoice choice)
    {
        if (program.Stage == Compute && program.WavePin is not null)
        {
            Warn(program.Name, $"has no form for this adapter ({choice.Reason}, waves {_adapter.WaveLaneCountMin}-{_adapter.WaveLaneCountMax}); not created");

            return null;
        }

        Warn(program.Name, $"may differ from the console on this adapter ({choice.Reason}); using the native DXIL");

        return program.Dxil;
    }

    private void Warn(string name, string message)
    {
        _warnings.Warn($"variant-{name}", $"[d3d12_xs] program {name} {message}");
    }
}
