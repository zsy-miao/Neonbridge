// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Cli.Shader;
using FiveStar.Formats.Rpf;
using FiveStar.Shaders.Cache;

namespace FiveStar.Cli.Setup;

/// <summary>
/// Reads the console's driver pipeline cache from shaders.rpf, matches its shaders to the extracted programs and writes
/// <c>&lt;work&gt;/pipelines/pipelines.json</c> and <c>code/</c> (runs <c>fivestar shader psocache</c>).
/// </summary>
internal sealed class PipelineCacheStep : ISetupStep
{
    public const string StepName = "pipeline-cache";

    private const string RecordsName = "pipelines.json";

    // The exported shader code and records take about 300 MiB.
    private const long ExpectedBytes = 512L << 20;

    private static readonly TimeSpan TypicalTime = TimeSpan.FromMinutes(1);

    public string Name => StepName;

    public string Title => "Read the console pipeline cache";

    public IReadOnlyList<string> DependsOn => [ShaderExtractStep.StepName];

    public TimeSpan Estimate(SetupContext context) => TypicalTime;

    public StepFingerprint Fingerprint(SetupContext context)
    {
        var inputs = Fingerprints.OfFiles(context.Dump, [context.ShaderArchive]);

        return new StepFingerprint(inputs, Fingerprints.OfAssemblies(typeof(PipelineCache), typeof(ChunkRunReader)));
    }

    public bool HasOutputs(SetupContext context) => File.Exists(Path.Combine(context.PipelineFolder, RecordsName));

    public long RequiredBytes(SetupContext context) => Math.Max(0, ExpectedBytes - StepText.FolderSize(context.PipelineFolder));

    public string? MissingRequirement(SetupContext context) => null;

    public StepResult Run(SetupContext context)
    {
        PipelineCacheCommand.Run([context.ShaderArchive, "--programs", context.ProgramFolder, "--out", context.PipelineFolder]);

        return StepResult.Done($"pipeline records and shader code in {context.PipelineFolder}");
    }
}
