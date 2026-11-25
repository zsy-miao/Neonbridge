// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Containers;

namespace FiveStar.Cli.Setup;

/// <summary>
/// Extracts the shader containers and native programs of shaders.rpf and the game executable to
/// <c>&lt;work&gt;/shaders/dxbc</c> and <c>fssm</c>.
/// </summary>
internal sealed class ShaderExtractStep : ISetupStep
{
    public const string StepName = "shader-extract";

    // The containers and programs take about 1.6 times the archive; doubled for headroom.
    private const long SpaceFactor = 2;

    private static readonly TimeSpan TypicalTime = TimeSpan.FromMinutes(1);

    public string Name => StepName;

    public string Title => "Extract shader programs";

    public IReadOnlyList<string> DependsOn => [VerifyStep.StepName];

    public TimeSpan Estimate(SetupContext context) => TypicalTime;

    public StepFingerprint Fingerprint(SetupContext context)
    {
        var inputs = Fingerprints.OfFiles(context.Dump, [context.ShaderArchive, .. Executable(context)]);

        return new StepFingerprint(inputs, Fingerprints.OfAssemblies(typeof(ShaderArchiveExtractor)));
    }

    public bool HasOutputs(SetupContext context) => File.Exists(context.ManifestPath) && Directory.Exists(context.ProgramFolder);

    public long RequiredBytes(SetupContext context) => new FileInfo(context.ShaderArchive).Length * SpaceFactor;

    public string? MissingRequirement(SetupContext context) => null;

    public StepResult Run(SetupContext context)
    {
        // A fresh folder, so programs of an earlier extraction never linger.
        foreach (var folder in new[] { ShaderArchiveExtractor.ContainerFolder, ShaderArchiveExtractor.ProgramFolder })
        {
            var path = Path.Combine(context.ShaderFolder, folder);

            if (Directory.Exists(path))
            {
                Directory.Delete(path, recursive: true);
            }
        }

        var summary = ShaderArchiveExtractor.Extract(context.ShaderArchive, context.ShaderFolder, Executable(context).FirstOrDefault());
        var text = $"{summary.Records} records, {summary.Containers} containers, {summary.Programs} programs";

        if (summary.ProgramFailures > 0)
        {
            text += $"; {summary.ProgramFailures} containers hold no readable program";
        }

        return StepResult.Done(text);
    }

    /// <summary>The game executable when the dump has it (its embedded programs are extracted too).</summary>
    private static string[] Executable(SetupContext context)
    {
        if (File.Exists(context.Executable))
        {
            return [context.Executable];
        }

        return [];
    }
}
