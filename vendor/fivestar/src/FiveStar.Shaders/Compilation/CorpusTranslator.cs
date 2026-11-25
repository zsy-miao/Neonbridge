// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Collections.Concurrent;
using System.Text.RegularExpressions;
using FiveStar.Shaders.Translation;

namespace FiveStar.Shaders.Compilation;

/// <summary>
/// Translates many FSSM programs and validates each with dxc: writes <c>&lt;name&gt;.hlsl</c> and <c>.dxil</c> (and the mesh
/// fallback pair <c>.msfb_cs</c>/<c>.msfb_vs</c> where needed or asked for, the wave32 variant <c>.w32</c> of wave64-pinned compute
/// programs and the superstep form <c>.w32</c> of mesh programs that need more lanes than D3D12's narrowest wave), the
/// runtime headers and results.json.
/// </summary>
public sealed partial class CorpusTranslator
{
    public const string ResultsName = "results.json";

    /// <summary>The file suffix of the wave32 variant of a wave64-pinned compute program.</summary>
    public const string GroupWaveSuffix = ".w32.hlsl";

    /// <summary>The reason of a wave64-pinned compute program whose wave32 variant compiled.</summary>
    public const string GroupWaveReady = GroupWaveNote + ReadySuffix;

    /// <summary>The reason of a mesh program whose fallback also compiled (<see cref="CorpusOptions.MeshFallbacks"/>).</summary>
    public const string MeshFallbackReady = MeshFallbackNote + ReadySuffix;

    /// <summary>The note of a mesh program whose superstep form also compiled as a fallback.</summary>
    public const string SuperstepFallbackReady = SuperstepFallbackNote + ReadySuffix;

    /// <summary>The note of a program whose native form no GPU of the run picks (<see cref="CorpusOptions.Waves"/>).</summary>
    public const string NativeSkipped = "native form not compiled";

    /// <summary>What a note of a form that compiled ends with.</summary>
    public const string ReadySuffix = " ok";

    /// <summary>Between the notes of a mesh program with a superstep form and a fallback.</summary>
    public const string ReasonSeparator = "; ";

    private const string Wave64Pin = "[WaveSize(64)]";

    // D3D12's narrowest wave: a mesh program with a higher floor also gets its superstep form.
    private const int NarrowestHardwareWave = 4;
    private const string GroupWaveNote = "wave32 variant";
    private const string MeshFallbackNote = "fallback";
    private const string SuperstepFallbackNote = "superstep fallback";

    private const int ShortReasonLength = 40;
    private const int CrashReasonLength = 80;
    private const int CompileReasonLength = 90;

    // Unsupported reasons that keep their detail; others are cut to their first word (the instruction).
    private static readonly string[] DetailedReasons = ["operand", "implicit", "export", "mask"];

    private readonly ProgramLoader _loader;
    private readonly IShaderCompiler? _compiler;
    private readonly MeshFallbackCompiler? _fallback;
    private readonly TranslationCache? _translations;
    private readonly IProgress<CorpusProgress>? _progress;
    private HashSet<string> _timedOut = [];

    /// <summary>
    /// Without a compiler the programs are only translated (status ok when the translation succeeds); without a
    /// translation cache every program is translated.
    /// </summary>
    public CorpusTranslator(
        ProgramLoader loader,
        IShaderCompiler? compiler,
        MeshFallbackCompiler? fallback,
        TranslationCache? translations,
        IProgress<CorpusProgress>? progress)
    {
        _loader = loader;
        _compiler = compiler;
        _fallback = fallback;
        _translations = translations;
        _progress = progress;
    }

    /// <summary>Runs every program (or, with a retry file, every program not final there) and writes results.json.</summary>
    public IReadOnlyList<ProgramResult> Run(IReadOnlyList<string> programPaths, CorpusOptions options)
    {
        Directory.CreateDirectory(options.OutputFolder);
        RuntimeIncludes.WriteTo(options.OutputFolder);
        IReadOnlyList<ProgramResult> prior = [];
        var paths = programPaths;

        if (options.RetryResults is not null)
        {
            prior = ResultsFile.Read(options.RetryResults);
            var final = prior.Where(result => ProgramStatus.IsFinal(result.Status)).Select(result => result.Name).ToHashSet();
            paths = programPaths.Where(path => !final.Contains(Path.GetFileNameWithoutExtension(path))).ToList();
        }

        var merged = Merge(prior, RunAll(paths, options));
        ResultsFile.Write(Path.Combine(options.OutputFolder, ResultsName), merged);

        return merged;
    }

    /// <summary>Runs the programs on the parallel jobs, the costliest first, and reports progress in expected cost.</summary>
    private ProgramResult[] RunAll(IReadOnlyList<string> paths, CorpusOptions options)
    {
        var results = new ProgramResult[paths.Count];
        var parallel = new ParallelOptions { MaxDegreeOfParallelism = Math.Max(1, options.Parallelism) };
        var costs = ExpectedCosts.Of(paths, options.OutputFolder);
        _timedOut = ExpectedCosts.TimedOut(options.OutputFolder);
        var total = costs.Sum();

        var done = 0;
        var finished = 0.0;
        var gate = new Lock();

        Parallel.ForEach(Partitioner.Create(CostliestFirst(costs), EnumerablePartitionerOptions.NoBuffering), parallel, index =>
        {
            results[index] = Work(paths[index], options);
            double work;

            lock (gate)
            {
                finished += costs[index];
                work = finished;
            }

            _progress?.Report(new CorpusProgress(Interlocked.Increment(ref done), paths.Count, work, total));
        });

        return results;
    }

    /// <summary>
    /// The indices of the programs, the costliest first (<see cref="ExpectedCosts"/>): dxc needs minutes for the largest
    /// few, so started last they would run on alone at the end of the run while the other jobs idle.
    /// </summary>
    private static int[] CostliestFirst(double[] costs)
    {
        return Enumerable.Range(0, costs.Length).OrderByDescending(index => costs[index]).ThenBy(index => index).ToArray();
    }

    /// <summary>Earlier results in their order with re-run programs replaced; new programs at the end.</summary>
    private static List<ProgramResult> Merge(IReadOnlyList<ProgramResult> prior, ProgramResult[] results)
    {
        var merged = prior.ToList();
        var positions = new Dictionary<string, int>();

        for (var index = 0; index < merged.Count; index++)
        {
            positions[merged[index].Name] = index;
        }

        foreach (var result in results)
        {
            if (positions.TryGetValue(result.Name, out var position))
            {
                merged[position] = result;
            }
            else
            {
                positions[result.Name] = merged.Count;
                merged.Add(result);
            }
        }

        return merged;
    }

    private ProgramResult Work(string path, CorpusOptions options)
    {
        var translated = Translate(path, options);
        string? stageName = null;

        if (translated.Stage is not null)
        {
            stageName = ShaderStages.ProfileName(translated.Stage.Value);
        }

        if (translated.Hlsl is null)
        {
            return new ProgramResult(translated.Name, stageName, translated.Status ?? ProgramStatus.Crash, translated.Reason);
        }

        var hlslPath = Path.Combine(options.OutputFolder, translated.Name + ".hlsl");
        File.WriteAllText(hlslPath, translated.Hlsl);
        StaticSampler.Keep(options.OutputFolder, translated.Hlsl);

        if (_compiler is null)
        {
            return new ProgramResult(translated.Name, stageName, ProgramStatus.Ok, null);
        }

        return CompileForms(_compiler, translated, stageName, hlslPath, options);
    }

    /// <summary>Compiles the forms of a translated program the run needs and its mesh fallbacks.</summary>
    private ProgramResult CompileForms(IShaderCompiler compiler, TranslatedProgram translated, string? stageName, string hlslPath, CorpusOptions options)
    {
        var (native, variant) = Forms(translated, options.Waves);
        var status = ProgramStatus.Ok;
        string? reason = null;
        var notes = new List<string>();

        if (native)
        {
            (status, reason) = Compile(compiler, hlslPath, translated.Hlsl!, translated);
        }
        else
        {
            // No GPU of the run picks it: an older run's DXIL of it must not stay behind.
            File.Delete(Path.ChangeExtension(hlslPath, ".dxil"));
            notes.Add(NativeSkipped);
        }

        if (status == ProgramStatus.Ok && translated.GroupWave is not null && variant)
        {
            var variantPath = Path.Combine(options.OutputFolder, translated.Name + GroupWaveSuffix);
            var note = CompileGroupWave(compiler, variantPath, translated, translated.GroupWave);
            notes.Add(note);

            if (!native && note != GroupWaveReady)
            {
                return new ProgramResult(translated.Name, stageName, ProgramStatus.CompileFailure, note);
            }
        }

        if (translated.Stage == ShaderStage.Mesh && _fallback is not null && status is ProgramStatus.Ok or ProgramStatus.OkMeshFallback)
        {
            notes.AddRange(MeshFallbacks(_fallback, options, status, hlslPath, translated));
        }

        if (notes.Count > 0)
        {
            reason = string.Join(ReasonSeparator, notes);
        }

        return new ProgramResult(translated.Name, stageName, status, reason);
    }

    /// <summary>
    /// The fallbacks of a mesh program, as notes for its reason: the plain one (with <see cref="CorpusOptions.MeshFallbacks"/>;
    /// an ok-msfb program has it already), and for a program that needs wide waves the plain compute shader pinned to the
    /// console wave and the fallback of its superstep form.
    /// </summary>
    private List<string> MeshFallbacks(MeshFallbackCompiler fallback, CorpusOptions options, string status, string hlslPath, TranslatedProgram program)
    {
        var notes = new List<string>();
        var hlsl = program.Hlsl!;
        var large = hlsl.Length > _compiler!.Options.LargeSourceLength;

        if (status == ProgramStatus.Ok)
        {
            if (!options.MeshFallbacks)
            {
                return notes;
            }

            var error = fallback.Compile(hlslPath, hlsl, large);
            notes.Add(FallbackReason(MeshFallbackNote, error));

            if (error is not null)
            {
                return notes;
            }
        }

        var wave = Analysis.WaveFacts.Read(hlsl).ProgramWave;

        if (!NeedsWideWaves(hlsl) || wave is null)
        {
            return notes;
        }

        notes.Add(FallbackReason(PinnedFallbackNote(wave.Value), fallback.CompilePinned(hlslPath, hlsl, large, wave.Value)));

        if (program.GroupWave?.Hlsl is not null)
        {
            var path = Path.Combine(options.OutputFolder, program.Name + GroupWaveSuffix);
            File.WriteAllText(path, program.GroupWave.Hlsl);
            notes.Add(FallbackReason(SuperstepFallbackNote, fallback.Compile(path, program.GroupWave.Hlsl, large)));
        }

        return notes;
    }

    /// <summary>
    /// Which of a program's native form and wave variant the run's GPUs (<see cref="CorpusOptions.Waves"/>) pick: a
    /// mesh program runs natively where the narrowest wave reaches its floor and as its superstep form elsewhere; a
    /// pinned compute program natively where the pin is a hardware wave size and as its wave32 variant elsewhere. Both
    /// without GPUs, for other programs and when the variant did not translate.
    /// </summary>
    private static (bool Native, bool Variant) Forms(TranslatedProgram program, IReadOnlyList<WaveRange>? waves)
    {
        if (waves is null || waves.Count == 0 || program.GroupWave?.Hlsl is null)
        {
            return (true, true);
        }

        var facts = Analysis.WaveFacts.Read(program.Hlsl!);
        var floor = facts.Floor;
        var pin = facts.WavePin;

        if (program.Stage == ShaderStage.Mesh && floor is not null)
        {
            return (waves.Any(wave => floor <= wave.Min), waves.Any(wave => floor > wave.Min));
        }

        if (program.Stage == ShaderStage.Compute && pin is not null)
        {
            return (waves.Any(wave => wave.Offers(pin.Value)), waves.Any(wave => !wave.Offers(pin.Value)));
        }

        return (true, true);
    }

    /// <summary>The note of a mesh fallback that was compiled: <c>&lt;note&gt; ok</c> or why it failed.</summary>
    private static string FallbackReason(string note, string? error)
    {
        if (error is null)
        {
            return note + ReadySuffix;
        }

        return Cut($"{note}: {error}", CompileReasonLength);
    }

    /// <summary>The note of the fallback compute shader pinned to <paramref name="lanes"/>-lane waves.</summary>
    public static string PinnedFallbackNote(int lanes)
    {
        return string.Create(System.Globalization.CultureInfo.InvariantCulture, $"wave{lanes} fallback");
    }

    /// <summary>The translation from the cache when it has one; otherwise a new one, kept when it is final.</summary>
    private TranslatedProgram Translate(string path, CorpusOptions options)
    {
        if (_translations is null)
        {
            return TranslateProgram(path, options);
        }

        var key = _translations.Key(path);
        var cached = _translations.Find(key, Path.GetFileNameWithoutExtension(path));

        if (cached is not null)
        {
            return SelectStage(cached, options.Stages);
        }

        var translated = TranslateProgram(path, options);

        if (translated.Hlsl is not null || translated.Status is ProgramStatus.Unsupported or ProgramStatus.Crash)
        {
            _translations.Store(key, translated);
        }

        return translated;
    }

    private static TranslatedProgram SelectStage(TranslatedProgram program, IReadOnlySet<ShaderStage>? stages)
    {
        if (stages is null || (program.Stage is not null && stages.Contains(program.Stage.Value)))
        {
            return program;
        }

        return program with { Hlsl = null, Status = ProgramStatus.SkipStage, Reason = null };
    }

    private TranslatedProgram TranslateProgram(string path, CorpusOptions options)
    {
        return TranslateFile(_loader, path, options.Stages, options.Translation);
    }

    /// <summary>
    /// Loads and translates one program, with its wave variant where it has one, as a run does before compiling; a
    /// program that does not translate has its status and reason instead of HLSL.
    /// </summary>
    public static TranslatedProgram TranslateFile(ProgramLoader loader, string path, IReadOnlySet<ShaderStage>? stages, TranslationOptions translation)
    {
        var loaded = loader.Load(path, stages);
        var program = loaded.Program;

        if (program is null)
        {
            return new TranslatedProgram(loaded.Name, loaded.Stage, null, loaded.Status ?? ProgramStatus.Crash, loaded.Reason, 0);
        }

        var count = program.Instructions.Count;

        try
        {
            var hlsl = HlslTranslator.Translate(program, translation).Hlsl;

            return new TranslatedProgram(loaded.Name, loaded.Stage, hlsl, null, null, count) { GroupWave = GroupWave(program, hlsl, translation) };
        }
        catch (UnsupportedProgramException exception)
        {
            return new TranslatedProgram(loaded.Name, loaded.Stage, null, ProgramStatus.Unsupported, UnsupportedReason(exception.Message), count);
        }
        catch (Exception exception) when (exception is not OutOfMemoryException)
        {
            var reason = Cut($"{exception.GetType().Name}: {exception.Message}", CrashReasonLength);

            return new TranslatedProgram(loaded.Name, loaded.Stage, null, ProgramStatus.Crash, reason, count);
        }
    }

    /// <summary>
    /// The variant of a wave64-pinned compute program for GPUs without 64-lane waves
    /// (<see cref="HlslTranslator.GroupWaveVariant"/>), from the structured form where the graph allows it (WARP's shader
    /// compiler crashes on barriers inside the dispatch switch); null for every other program.
    /// </summary>
    private static GroupWaveVariant? GroupWave(ShaderProgram program, string hlsl, TranslationOptions options)
    {
        if (program.Stage == ShaderStage.Mesh)
        {
            return MeshSupersteps(program, hlsl, options);
        }

        if (program.Stage != ShaderStage.Compute || !hlsl.Contains(Wave64Pin, StringComparison.Ordinal))
        {
            return null;
        }

        try
        {
            var structured = HlslTranslator.Translate(program, options with { StructuredControlFlow = true }).Hlsl;

            return new GroupWaveVariant(HlslTranslator.GroupWaveVariant(structured), null);
        }
        catch (UnsupportedProgramException)
        {
            // Several console waves in the group, or a wave operation under a per-lane condition: supersteps.
            return Supersteps(program, options);
        }
    }

    private static GroupWaveVariant Supersteps(ShaderProgram program, TranslationOptions options)
    {
        try
        {
            return new GroupWaveVariant(HlslTranslator.SuperstepVariant(program, options), null);
        }
        catch (UnsupportedProgramException exception)
        {
            return new GroupWaveVariant(null, Cut($"{GroupWaveNote}: {exception.Message}", CompileReasonLength));
        }
    }

    /// <summary>
    /// The superstep form of a mesh program whose floor is above the narrowest hardware wave
    /// (<see cref="HlslTranslator.SuperstepVariant"/>): console results on any wave; null for the others.
    /// </summary>
    private static GroupWaveVariant? MeshSupersteps(ShaderProgram program, string hlsl, TranslationOptions options)
    {
        if (!NeedsWideWaves(hlsl))
        {
            return null;
        }

        return Supersteps(program, options);
    }

    /// <summary>Whether a mesh program's floor is above the narrowest hardware wave.</summary>
    private static bool NeedsWideWaves(string hlsl)
    {
        var floor = Analysis.WaveFacts.Read(hlsl).Floor;

        return floor is not null && floor > NarrowestHardwareWave;
    }

    /// <summary>Writes and compiles <c>name.w32.hlsl</c>; the outcome becomes the reason of the ok result.</summary>
    private string CompileGroupWave(IShaderCompiler compiler, string path, TranslatedProgram program, GroupWaveVariant variant)
    {
        if (variant.Hlsl is null)
        {
            return variant.Reason ?? GroupWaveNote;
        }

        File.WriteAllText(path, variant.Hlsl);
        var (status, reason) = Compile(compiler, path, variant.Hlsl, program);

        if (status == ProgramStatus.Ok)
        {
            return GroupWaveReady;
        }

        return Cut($"{GroupWaveNote}: {status} {reason}", CompileReasonLength);
    }

    private (string Status, string? Reason) Compile(IShaderCompiler compiler, string hlslPath, string hlsl, TranslatedProgram program)
    {
        var profile = ShaderStages.ProfileName(program.Stage!.Value);
        var large = hlsl.Length > compiler.Options.LargeSourceLength || _timedOut.Contains(Path.GetFileName(hlslPath));
        var outcome = compiler.Compile(new CompileRequest(hlslPath, hlsl, profile, large), compiler.Options.CompileTimeout);

        if (outcome.TimedOut)
        {
            outcome = compiler.Compile(new CompileRequest(hlslPath, hlsl, profile, DisableOptimizations: true), compiler.Options.RecompileTimeout);
        }

        if (outcome.TimedOut)
        {
            return (ProgramStatus.CompileTimeout, $"{program.Instructions} instructions");
        }

        if (outcome.Succeeded)
        {
            return (ProgramStatus.Ok, null);
        }

        var error = DxcCompiler.FirstError(outcome.Error);

        if (program.Stage == ShaderStage.Mesh && _fallback is not null)
        {
            // PC mesh output limit (32 KB) and similar: compute + vertex-pulling fallback.
            var fallbackError = _fallback.Compile(hlslPath, hlsl, large);

            if (fallbackError is null)
            {
                return (ProgramStatus.OkMeshFallback, null);
            }

            error = "fallback: " + fallbackError;
        }

        return (ProgramStatus.CompileFailure, Cut(ErrorPrefixPattern().Replace(error, string.Empty, 1), CompileReasonLength));
    }

    private static string UnsupportedReason(string message)
    {
        if (DetailedReasons.Any(prefix => message.StartsWith(prefix, StringComparison.Ordinal)))
        {
            return Cut(message, ShortReasonLength);
        }

        return message.Split((char[]?)null, StringSplitOptions.RemoveEmptyEntries).FirstOrDefault() ?? message;
    }

    private static string Cut(string text, int length)
    {
        if (text.Length <= length)
        {
            return text;
        }

        return text[..length];
    }

    [GeneratedRegex(@"^.*?error: ")]
    private static partial Regex ErrorPrefixPattern();
}
