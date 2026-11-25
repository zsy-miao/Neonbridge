// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using FiveStar.Cli.Shader;
using FiveStar.Graphics.Programs;
using FiveStar.Shaders.Compilation;

namespace FiveStar.Cli.Setup;

/// <summary>
/// Compiles the rectangle-list geometry shaders of the translated vertex programs into
/// <c>&lt;work&gt;/shaders/hlsl/rect-gs</c> (runs <c>fivestar shader rect-gs</c>). A shader already compiled is kept, so a
/// re-run after new programs compiles only the new output signatures.
/// </summary>
internal sealed class RectangleShaderStep : ISetupStep
{
    public const string StepName = "rect-gs";

    private const string CompiledPattern = "*.dxil";

    private static readonly TimeSpan TypicalTime = TimeSpan.FromMinutes(1);

    private string? _tool;

    public string Name => StepName;

    public string Title => "Compile rectangle-list geometry shaders";

    public IReadOnlyList<string> DependsOn => [ShaderTranslateStep.StepName];

    public TimeSpan Estimate(SetupContext context) => TypicalTime;

    public StepFingerprint Fingerprint(SetupContext context)
    {
        _tool ??= Fingerprints.Combine(ShaderTranslateStep.Identity(context.Arguments), Fingerprints.OfAssemblies(typeof(RectangleGeometryShader)));

        return new StepFingerprint(Fingerprints.OfContent(Path.Combine(context.TranslationFolder, ShaderManifest.FileName)), _tool);
    }

    public bool HasOutputs(SetupContext context) => Directory.Exists(Folder(context));

    public long RequiredBytes(SetupContext context) => 0;

    public string? MissingRequirement(SetupContext context)
    {
        var missing = ToolCheck.MissingShaderTools(context.Arguments);

        if (missing.Count == 0)
        {
            return null;
        }

        return string.Join("; ", missing);
    }

    public StepResult Run(SetupContext context)
    {
        var arguments = new List<string> { context.TranslationFolder, "--jobs", context.Jobs.ToString(CultureInfo.InvariantCulture) };
        var dxc = context.Arguments.Option("dxc");

        if (dxc is not null)
        {
            arguments.AddRange(["--dxc", dxc]);
        }

        if (RectangleShaderCommand.Run(arguments) != 0)
        {
            return StepResult.Failed("some geometry shaders did not compile (listed above); a re-run retries them");
        }

        var count = Directory.EnumerateFiles(Folder(context), CompiledPattern).Count();

        return StepResult.Done($"{count} geometry shaders in {Folder(context)}");
    }

    private static string Folder(SetupContext context) => Path.Combine(context.TranslationFolder, RectangleGeometryShader.Folder);
}
