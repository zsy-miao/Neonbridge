// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Cli.Setup;

namespace FiveStar.Cli.Run;

/// <summary>
/// Everything one <c>fivestar run</c> works from: the options, the setup context of the work folder, the dump's
/// identity, the loader found (or null), the setup steps the run needs with their condition now, and the free space of
/// the work folder's drive.
/// </summary>
internal sealed record RunPlan(
    RunOptions Options,
    SetupContext Context,
    DumpIdentity Identity,
    string? LoaderPath,
    IReadOnlyList<ISetupStep> Steps,
    IReadOnlyList<StepCondition> Conditions,
    long FreeBytes)
{
    /// <summary>The game executable in the dump folder.</summary>
    public const string GameExecutable = "game_scarlett_master.exe";

    public string Executable => Path.Combine(Options.Dump, GameExecutable);

    /// <summary>The extracted FSSM programs the loader reads (<c>--shader-dir</c>).</summary>
    public string ProgramFolder => Options.Programs ?? Context.ProgramFolder;

    /// <summary>The translated DXIL the loader reads (<c>--translated-dir</c>).</summary>
    public string TranslatedFolder => Options.Translated ?? Context.TranslationFolder;

    /// <summary>The setup steps that will run before the loader starts.</summary>
    public IEnumerable<ISetupStep> PendingSteps => Steps.Where((_, index) => Conditions[index] != StepCondition.Done);
}
