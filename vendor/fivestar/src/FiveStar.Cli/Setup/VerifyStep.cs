// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Cli.Setup;

/// <summary>
/// Checks that the folder is the dump the project targets: the title id and version of MicrosoftGame.Config, and the
/// files the later steps read (shaders.rpf, the texture page files). Another version is accepted with a warning.
/// </summary>
internal sealed class VerifyStep : ISetupStep
{
    public const string StepName = "verify";

    // Bumped by hand when the checks change; the later steps depend on this step's record.
    private const string Version = "1";

    private static readonly TimeSpan TypicalTime = TimeSpan.FromSeconds(1);

    public string Name => StepName;

    public string Title => "Verify the dump";

    public IReadOnlyList<string> DependsOn => [];

    public TimeSpan Estimate(SetupContext context) => TypicalTime;

    public StepFingerprint Fingerprint(SetupContext context)
    {
        var files = new List<string> { Path.Combine(context.Dump, DumpIdentity.ConfigName), context.ShaderArchive };
        files.AddRange(TextureStep.PageFiles(context.Dump));

        return new StepFingerprint(Fingerprints.OfFiles(context.Dump, files), Version);
    }

    public bool HasOutputs(SetupContext context) => true;

    public long RequiredBytes(SetupContext context) => 0;

    public string? MissingRequirement(SetupContext context) => null;

    public StepResult Run(SetupContext context)
    {
        var identity = DumpIdentity.Read(context.Dump);

        if (identity.TitleId != DumpIdentity.ExpectedTitleId)
        {
            return StepResult.Failed($"title id {identity.TitleId} is not GTA VI ({DumpIdentity.ExpectedTitleId})");
        }

        if (!File.Exists(context.ShaderArchive))
        {
            return StepResult.Failed($"shaders.rpf is missing from {context.Dump}; the dump is incomplete");
        }

        var pages = TextureStep.PageFiles(context.Dump);

        if (pages.Count == 0)
        {
            return StepResult.Failed($"no texture page files (data/ttdb/*.ttpf) in {context.Dump}; the dump is incomplete");
        }

        var summary = $"{identity.Name} {identity.TitleId} version {identity.Version}, {pages.Count} texture page files";

        if (identity.Version != DumpIdentity.ExpectedVersion)
        {
            summary += $"; warning: the project is checked against version {DumpIdentity.ExpectedVersion}";
        }

        return StepResult.Done(summary);
    }
}
