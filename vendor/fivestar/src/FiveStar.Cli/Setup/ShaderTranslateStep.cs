// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Cli.Graphics;
using System.Globalization;
using FiveStar.Cli.Shader;
using FiveStar.Graphics;
using FiveStar.Shaders.Cache;
using FiveStar.Graphics.Programs;
using FiveStar.Shaders.Compilation;
using FiveStar.Shaders.Translation;

namespace FiveStar.Cli.Setup;

/// <summary>
/// Translates every extracted program to HLSL and compiles it with dxc through the shader cache: runs
/// <c>fivestar shader translate</c> (with <c>--mesh-fallbacks</c> where a GPU needs them) into <c>&lt;work&gt;/shaders/hlsl</c> with <c>--cache &lt;work&gt;/shader-cache</c>,
/// which also writes the variant manifest <c>variants.json</c> there. An
/// interrupted run keeps every finished translation and compile in the cache, so the next run resumes where it stopped.
/// </summary>
internal sealed class ShaderTranslateStep : ISetupStep
{
    public const string StepName = "shader-translate";

    /// <summary>
    /// The translator options setup uses: structured control flow with pinned and emulated wave sizes, as the translate
    /// command.
    /// </summary>
    public static readonly TranslationOptions Translation = new(StructuredControlFlow: true, PinWaveSize: true, EmulateWaveSize: true);

    // Measured: HLSL, DXIL and cache entries come to about 610 KiB per program.
    private const long BytesPerProgram = 640 * 1024;

    // Measured: a cold translation and compile takes about 3.2 s of one job per program.
    private const double JobSecondsPerProgram = 3.2;

    // The programs of version 1.0.2.1, for the estimate before they are extracted.
    private const int CorpusPrograms = 25_884;

    private const string ProgramPattern = "*.fssm";

    /// <summary>The setup option that asks for mesh fallbacks whatever the GPUs.</summary>
    public const string MeshFallbacksFlag = "mesh-fallbacks";

    /// <summary>The setup option that compiles every form of every program, whatever the GPUs (for another machine).</summary>
    public const string AllFormsFlag = "all-forms";

    private static readonly string[] ToolOptions = ["llvm", "dxc"];

    private string? _tool;
    private List<AdapterInfo>? _gpus;

    public string Name => StepName;

    public string Title => "Translate and compile shaders";

    public IReadOnlyList<string> DependsOn => [ShaderExtractStep.StepName];

    public TimeSpan Estimate(SetupContext context) => TimeSpan.FromSeconds(Programs(context) * JobSecondsPerProgram / context.Jobs);

    /// <summary>The identity of the translator and compiler build: also the key of the cache's usage record.</summary>
    public static string Identity(CommandArguments arguments)
    {
        var compiler = ShaderTools.Compiler(arguments, retry: false);

        var (_, llvm) = ShaderTools.Disassembler(arguments);

        return ShaderCacheIdentity.Of(llvm, Translation, compiler);
    }

    public StepFingerprint Fingerprint(SetupContext context)
    {
        _tool ??= Identity(context.Arguments);

        return new StepFingerprint(Fingerprints.Combine(Fingerprints.OfContent(context.ManifestPath), context.Sample, Fallbacks(context).ToString(), Waves(context) ?? AllFormsFlag), _tool);
    }

    public bool HasOutputs(SetupContext context) =>
        File.Exists(ResultsPath(context)) && File.Exists(Path.Combine(context.TranslationFolder, ShaderManifest.FileName));

    public long RequiredBytes(SetupContext context)
    {
        // What an earlier run wrote is overwritten in place or reused from the cache.
        var existing = StepText.FolderSize(context.TranslationFolder) + StepText.FolderSize(context.CacheFolder);

        return Math.Max(0, (Programs(context) * BytesPerProgram) - existing);
    }

    /// <summary>The programs the sample selects: the first <c>--limit</c> of every <c>--step</c>-th.</summary>
    private static long Programs(SetupContext context)
    {
        long programs = CorpusPrograms;

        if (Directory.Exists(context.ProgramFolder))
        {
            programs = Directory.EnumerateFiles(context.ProgramFolder, ProgramPattern).Count();
        }

        programs = (programs + context.Every - 1) / context.Every;

        if (context.Limit is not null)
        {
            programs = Math.Min(programs, context.Limit.Value);
        }

        return programs;
    }

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
        TranslateCommand.Run(Arguments(context));
        var results = ResultsFile.Read(ResultsPath(context));
        var counts = results.GroupBy(result => result.Status).OrderByDescending(group => group.Count()).Select(group => $"{group.Key} {group.Count()}");
        var summary = $"{results.Count} programs: {string.Join(", ", counts)}";
        var open = results.Count(result => !ProgramStatus.IsFinal(result.Status));

        if (open > 0)
        {
            summary += $"; {open} did not translate or compile (see {ResultsPath(context)})";
        }

        return StepResult.Done(summary);
    }

    /// <summary>
    /// Whether every mesh program also gets its compute + vertex-pulling fallback: with <c>--mesh-fallbacks</c>, or when
    /// an adapter of this machine has no mesh shaders. Mesh-shader GPUs never load them, and they double the mesh
    /// programs' dxc work; a program dxc rejects as a mesh shader gets its fallback either way.
    /// </summary>
    private bool Fallbacks(SetupContext context)
    {
        return context.Arguments.Flag(MeshFallbacksFlag) || Gpus().Any(adapter => !adapter.MeshShaders);
    }

    /// <summary>
    /// The wave sizes of this machine's GPUs for <c>--waves</c>, so only the forms they pick are compiled (a NVIDIA GPU
    /// never runs the native form of a mesh program whose floor is above 32 lanes); null with <c>--all-forms</c> or
    /// without a GPU, which compiles every form.
    /// </summary>
    private string? Waves(SetupContext context)
    {
        var gpus = Gpus();

        if (context.Arguments.Flag(AllFormsFlag) || gpus.Count == 0)
        {
            return null;
        }

        return string.Join(",", gpus.Select(adapter => new WaveRange(adapter.WaveLaneCountMin, adapter.WaveLaneCountMax).ToString()).Distinct());
    }

    /// <summary>The hardware adapters with a D3D12 device.</summary>
    private List<AdapterInfo> Gpus()
    {
        _gpus ??= MeasuredAdapters.List().Where(adapter => !adapter.Software && adapter.ShaderModel is not null).ToList();

        return _gpus;
    }

    private static string ResultsPath(SetupContext context)
    {
        return Path.Combine(context.TranslationFolder, CorpusTranslator.ResultsName);
    }

    private List<string> Arguments(SetupContext context)
    {
        var jobs = context.Jobs.ToString(CultureInfo.InvariantCulture);
        var arguments = new List<string> { context.ProgramFolder, context.TranslationFolder, "--cache", context.CacheFolder, "--jobs", jobs };

        if (Fallbacks(context))
        {
            arguments.Add("--" + MeshFallbacksFlag);
        }

        var waves = Waves(context);

        if (waves is not null)
        {
            arguments.AddRange(["--waves", waves]);
        }

        // The programs earlier boots used go first (the loader's journal beside its pipeline cache).
        var journal = Path.Combine(context.PipelineLibraryFolder, PipelineJournalFile.FileName);

        if (File.Exists(journal))
        {
            arguments.AddRange(["--first-used", journal]);
        }

        if (context.Limit is not null)
        {
            arguments.AddRange(["--limit", context.Limit.Value.ToString(CultureInfo.InvariantCulture)]);
        }

        arguments.AddRange(["--step", context.Every.ToString(CultureInfo.InvariantCulture)]);

        foreach (var name in ToolOptions)
        {
            var value = context.Arguments.Option(name);

            if (value is not null)
            {
                arguments.AddRange(["--" + name, value]);
            }
        }

        return arguments;
    }
}
