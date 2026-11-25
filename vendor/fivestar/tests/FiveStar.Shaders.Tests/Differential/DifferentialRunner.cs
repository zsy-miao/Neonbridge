// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Diagnostics;
using System.Globalization;
using FiveStar.Shaders.Compilation;
using FiveStar.Shaders.Disassembly;
using FiveStar.Shaders.Dxil;
using FiveStar.Shaders.Tests.Toolchain;
using FiveStar.Shaders.Tests.Warp;
using FiveStar.Shaders.Translation;

namespace FiveStar.Shaders.Tests.Differential;

/// <summary>
/// Runs a real program in the dispatch-loop and the structured control-flow form on WARP with the same synthetic
/// inputs (<see cref="SyntheticData"/>, heap indices redirected by <see cref="HeapSlots"/>, outputs captured by
/// <see cref="StageHarness"/>) and compares every output word. Translation and dxc run on the calling thread; the WARP
/// runs of all threads take turns, so several programs can be prepared at once.
/// </summary>
public sealed class DifferentialRunner
{
    private const int MeshVertexBytes = 1 << 16;
    private const int MeshIndexBytes = 1 << 14;
    // The table's t0 (the vertex-pulling VS reads it; the compute half does not).
    private const int VertexViewWords = 8;
    private const int TargetSize = 16;
    private const int DetailLength = 300;
    private const int Seeds = 3;
    private const int SeedStep = 7919;
    private const string LoopBudgetNote = "loop budget";

    /// <summary>The detail of programs not run because WARP is still busy with an earlier one.</summary>
    public const string NotRunNote = "not run: WARP stalled on";

    // WARP's shader compiler needs minutes and many GB for larger dispatch-form programs (90 KB of HLSL: over 10 min, 13 GB).
    private const int MaxSource = 64_000;

    // The structured form compiles quickly on WARP; the DXIL backend comparison takes larger programs.
    private const int MaxBackendSource = 400_000;

    private static readonly TranslationOptions Dispatch = new(StructuredControlFlow: false, PinWaveSize: false, EmulateWaveSize: true);

    private readonly WarpFixture _fixture;
    private readonly ShaderWorkbench _workbench;
    private readonly string _folder;
    private readonly bool _backend;
    private readonly Lock _gpu = new();

    // The program whose run WARP did not finish; WARP cannot cancel it, so nothing else runs after it.
    private string? _stalled;

    /// <summary>
    /// Builds in <paramref name="folder"/>, whose prelude gets the same heap redirection as the programs. With
    /// <paramref name="backend"/> it compares the structured form compiled by dxc with the same form compiled by the DXIL
    /// backend (compute programs) instead of the two control-flow forms.
    /// </summary>
    public DifferentialRunner(WarpFixture fixture, string folder, bool backend)
    {
        _fixture = fixture;
        _folder = folder;
        _backend = backend;
        _workbench = new ShaderWorkbench(ShaderToolchain.Find(), folder) { Header = fixture.Workbench.Header };
        var prelude = Path.Combine(folder, RuntimeIncludes.Names[0]);
        File.WriteAllText(prelude, HeapSlots.Rewrite(RuntimeIncludes.Read(RuntimeIncludes.Names[0])));
    }

    public DifferentialOutcome Run(string path)
    {
        var name = Path.GetFileNameWithoutExtension(path);
        var loaded = new ProgramLoader(new RdnaDecoder()).Load(path, null);

        if (loaded.Program is null)
        {
            return new DifferentialOutcome(name, "?", DifferentialVerdict.Skipped, $"load: {loaded.Status} {loaded.Reason}");
        }

        var program = loaded.Program;
        var stage = ShaderStages.ProfileName(program.Stage);

        try
        {
            return Compare(name, stage, program);
        }
        catch (UnsupportedProgramException exception)
        {
            return new DifferentialOutcome(name, stage, DifferentialVerdict.Skipped, Cut("unsupported: " + exception.Message));
        }
        catch (NotSupportedException exception)
        {
            return new DifferentialOutcome(name, stage, DifferentialVerdict.Skipped, Cut("harness: " + exception.Message));
        }
    }

    /// <summary>
    /// A mesh program in the form --fast-waves gives it (the native form, approximate on narrower waves) against its
    /// superstep form (console results), both through the compute fallback on WARP's 4-lane waves, the narrowest there
    /// is. Skipped when fast waves leave the program as it was.
    /// </summary>
    public DifferentialOutcome RunFastWaves(string path)
    {
        var name = Path.GetFileNameWithoutExtension(path);
        var program = new ProgramLoader(new RdnaDecoder()).Load(path, null).Program;

        if (program is null || program.Stage != ShaderStage.Mesh)
        {
            return new DifferentialOutcome(name, "ms", DifferentialVerdict.Skipped, "not a mesh program");
        }

        try
        {
            var fast = HlslTranslator.Translate(program, Dispatch with { FastWaves = true }).Hlsl;

            if (!fast.Contains(TranslationOptions.FastWavesComment, StringComparison.Ordinal))
            {
                return new DifferentialOutcome(name, "ms", DifferentialVerdict.Skipped, "not a fast-waves program");
            }

            var superstep = HlslTranslator.SuperstepVariant(program, Dispatch);

            if (superstep.Length > MaxBackendSource)
            {
                return new DifferentialOutcome(name, "ms", DifferentialVerdict.Skipped, $"large: {superstep.Length} characters");
            }

            var first = Prepare(name + ".fast", ShaderStage.Mesh, fast, string.Empty);
            var second = Prepare(name + ".superstep", ShaderStage.Mesh, superstep, string.Empty);

            if (first.Error is not null || second.Error is not null)
            {
                return CompileOutcome(name, "ms", first.Error, second.Error);
            }

            lock (_gpu)
            {
                if (_stalled is not null)
                {
                    return new DifferentialOutcome(name, "ms", DifferentialVerdict.Skipped, $"{NotRunNote} {_stalled}");
                }

                return Execute(name, "ms", first, second, StableSeed(name));
            }
        }
        catch (UnsupportedProgramException exception)
        {
            return new DifferentialOutcome(name, "ms", DifferentialVerdict.Skipped, Cut("unsupported: " + exception.Message));
        }
    }

    private DifferentialOutcome Compare(string name, string stage, ShaderProgram program)
    {
        if (_backend)
        {
            return CompareBackend(name, stage, program);
        }

        var dispatch = HlslTranslator.Translate(program, Dispatch).Hlsl;
        var structured = HlslTranslator.Translate(program, Dispatch with { StructuredControlFlow = true }).Hlsl;

        if (dispatch == structured || program.Stage == ShaderStage.Geometry)
        {
            var verdict = DifferentialVerdict.Skipped;

            if (dispatch == structured)
            {
                verdict = DifferentialVerdict.SameText;
            }

            return new DifferentialOutcome(name, stage, verdict, Cut(FallbackReason(structured)));
        }

        if (dispatch.Length > MaxSource)
        {
            return new DifferentialOutcome(name, stage, DifferentialVerdict.Skipped, $"large: {dispatch.Length} characters");
        }

        var seed = StableSeed(name);
        var feeder = string.Empty;

        if (program.Stage == ShaderStage.Pixel)
        {
            feeder = StageHarness.PixelFeeder(dispatch, new SyntheticData(seed + 1));
        }

        var first = Prepare(name + ".dispatch", program.Stage, dispatch, feeder);
        var second = Prepare(name + ".structured", program.Stage, structured, feeder);

        if (first.Error is not null || second.Error is not null)
        {
            return CompileOutcome(name, stage, first.Error, second.Error);
        }

        lock (_gpu)
        {
            if (_stalled is not null)
            {
                return new DifferentialOutcome(name, stage, DifferentialVerdict.Skipped, $"{NotRunNote} {_stalled}");
            }

            return Execute(name, stage, first, second, seed);
        }
    }

    /// <summary>
    /// The structured form through dxc and through the DXIL backend: compute programs as they are, pixel shaders in their
    /// harness (the feeder vertex shader from dxc), vertex shaders through their compute harness.
    /// </summary>
    private DifferentialOutcome CompareBackend(string name, string stage, ShaderProgram program)
    {
        var structured = HlslTranslator.Translate(program, Dispatch with { StructuredControlFlow = true }).Hlsl;

        if (program.Stage is not (ShaderStage.Compute or ShaderStage.Pixel or ShaderStage.Vertex) || structured.Length > MaxBackendSource)
        {
            return new DifferentialOutcome(name, stage, DifferentialVerdict.Skipped, $"not compared: {stage}, {structured.Length} characters");
        }

        PreparedForm first;

        if (program.Stage == ShaderStage.Compute)
        {
            first = PrepareStructured(name + ".dxc", structured);
        }
        else
        {
            var feeder = string.Empty;

            if (program.Stage == ShaderStage.Pixel)
            {
                feeder = StageHarness.PixelFeeder(structured, new SyntheticData(StableSeed(name) + 1));
            }

            first = Prepare(name + ".dxc", program.Stage, structured, feeder);
        }

        var second = PrepareBackend(first with { Name = name + ".dxil", Shader = [] });

        if (first.Error is not null || second.Error is not null)
        {
            return CompileOutcome(name, stage, first.Error, second.Error);
        }

        lock (_gpu)
        {
            if (_stalled is not null)
            {
                return new DifferentialOutcome(name, stage, DifferentialVerdict.Skipped, $"{NotRunNote} {_stalled}");
            }

            return Execute(name, stage, first, second, StableSeed(name));
        }
    }

    /// <summary>The structured compute form through dxc (a group barrier is fine outside the dispatch switch).</summary>
    private PreparedForm PrepareStructured(string name, string hlsl)
    {
        var source = LoopBudget.Insert(HeapSlots.Rewrite(hlsl));

        try
        {
            return new PreparedForm(name, ShaderStage.Compute, source, "cs", _workbench.Compile(name, source, "cs"), null, 0, 0, null);
        }
        catch (InvalidOperationException exception)
        {
            return new PreparedForm(name, ShaderStage.Compute, source, "cs", [], null, 0, 0, exception.Message.Split('\n')[0]);
        }
    }

    /// <summary>The source of a dxc form (already rewritten and wrapped) compiled by the DXIL backend.</summary>
    private PreparedForm PrepareBackend(PreparedForm form)
    {
        if (form.Error is not null)
        {
            return form;
        }

        try
        {
            return form with { Shader = DxilBackend.Compile(_workbench.Header + form.Source, form.Profile, include => File.ReadAllText(Path.Combine(_folder, include))) };
        }
        catch (DxilBackendException exception)
        {
            return form with { Error = "dxil backend: " + exception.Message };
        }
    }

    /// <summary>Runs both forms with each of <see cref="Seeds"/> input sets; the first seed that differs decides.</summary>
    private DifferentialOutcome Execute(string name, string stage, PreparedForm dispatch, PreparedForm structured, int seed)
    {
        var clock = Stopwatch.StartNew();
        var (written, exhausted) = (0, 0);

        for (var index = 0; index < Seeds; index++)
        {
            var outcome = ExecuteSeed(name, stage, dispatch, structured, seed + (index * SeedStep));

            if (outcome.Verdict == DifferentialVerdict.Skipped && outcome.Detail == LoopBudgetNote)
            {
                exhausted++;

                continue;
            }

            if (outcome.Verdict != DifferentialVerdict.Agree)
            {
                return outcome;
            }

            written += int.Parse(outcome.Detail, CultureInfo.InvariantCulture);
        }

        var timing = string.Create(CultureInfo.InvariantCulture, $"WARP {clock.ElapsedMilliseconds} ms");

        if (exhausted == Seeds)
        {
            return new DifferentialOutcome(name, stage, DifferentialVerdict.Skipped, $"{LoopBudgetNote}; {timing}");
        }

        var detail = string.Create(CultureInfo.InvariantCulture, $"{written} words written, {Seeds - exhausted} input sets; {timing}");

        return new DifferentialOutcome(name, stage, DifferentialVerdict.Agree, detail);
    }

    /// <summary>One input set: Agree with the written word count as detail, or the verdict that ends the comparison.</summary>
    private DifferentialOutcome ExecuteSeed(string name, string stage, PreparedForm dispatch, PreparedForm structured, int seed)
    {
        var (dispatchResult, dispatchError) = Launch(dispatch, seed);
        (WarpResult? Result, string? Error) second = (null, null);

        if (!IsStall(dispatchError))
        {
            second = Launch(structured, seed);
        }

        var (structuredResult, structuredError) = second;

        if (dispatchError is not null || structuredError is not null)
        {
            return RunFailure(name, stage, dispatchError, structuredError);
        }

        if (dispatchResult!.Buffers[LoopBudget.FlagSlot][0] != 0 || structuredResult!.Buffers[LoopBudget.FlagSlot][0] != 0)
        {
            return new DifferentialOutcome(name, stage, DifferentialVerdict.Skipped, LoopBudgetNote);
        }

        var difference = OutputComparison.Describe(dispatchResult, structuredResult);

        if (difference is null)
        {
            var written = OutputComparison.Written(dispatchResult, new SyntheticData(seed).Bindings([]));

            return new DifferentialOutcome(name, stage, DifferentialVerdict.Agree, written.ToString(CultureInfo.InvariantCulture));
        }

        // A race (LDS shared across WARP's 4-lane waves without a barrier) shows as a form that differs from itself.
        var (dispatchAgain, _) = Launch(dispatch, seed);
        var (structuredAgain, _) = Launch(structured, seed);

        if (dispatchAgain is null || structuredAgain is null
            || OutputComparison.Describe(dispatchResult, dispatchAgain) is not null
            || OutputComparison.Describe(structuredResult, structuredAgain) is not null)
        {
            return new DifferentialOutcome(name, stage, DifferentialVerdict.Nondeterministic, Cut(difference));
        }

        if (AgreeStrict(dispatch, structured, seed))
        {
            return new DifferentialOutcome(name, stage, DifferentialVerdict.FastMath, Cut($"seed {seed}: {difference}"));
        }

        return new DifferentialOutcome(name, stage, DifferentialVerdict.Mismatch, Cut($"seed {seed}: {difference}"));
    }

    /// <summary>A run that did not finish: a WARP stall stops all later runs; one form failing alone is a mismatch.</summary>
    private DifferentialOutcome RunFailure(string name, string stage, string? dispatchError, string? structuredError)
    {
        if (IsStall(dispatchError) || IsStall(structuredError))
        {
            _stalled = name;

            return new DifferentialOutcome(name, stage, DifferentialVerdict.Skipped, Cut($"WARP stalled: dispatch {dispatchError ?? "ok"}; structured {structuredError ?? "not run"}"));
        }

        if (dispatchError is not null && structuredError is not null)
        {
            return new DifferentialOutcome(name, stage, DifferentialVerdict.Skipped, Cut($"run: {dispatchError}"));
        }

        return new DifferentialOutcome(name, stage, DifferentialVerdict.Mismatch, Cut($"run: dispatch {dispatchError ?? "ok"}; structured {structuredError ?? "ok"}"));
    }

    /// <summary>Whether the forms agree once both are compiled with IEEE strictness (dxc -Gis).</summary>
    private bool AgreeStrict(PreparedForm dispatch, PreparedForm structured, int seed)
    {
        try
        {
            var first = dispatch with { Shader = _workbench.CompileStrict(dispatch.Name, dispatch.Source, dispatch.Profile) };
            var second = structured;

            if (!_backend)
            {
                second = structured with { Shader = _workbench.CompileStrict(structured.Name, structured.Source, structured.Profile) };
            }

            var (firstResult, _) = Launch(first, seed);
            var (secondResult, _) = Launch(second, seed);

            return firstResult is not null && secondResult is not null && OutputComparison.Describe(firstResult, secondResult) is null;
        }
        catch (InvalidOperationException)
        {
            return false;
        }
    }

    private PreparedForm Prepare(string name, ShaderStage stage, string hlsl, string feeder)
    {
        var rewritten = LoopBudget.Insert(HeapSlots.Rewrite(hlsl));
        var failed = new PreparedForm(name, stage, string.Empty, string.Empty, [], null, 0, 0, null);

        try
        {
            var form = Wrap(name, stage, rewritten);
            byte[]? feederShader = null;

            if (stage == ShaderStage.Pixel)
            {
                feederShader = _workbench.Compile(name + ".feeder", feeder, "vs");
            }

            return form with { Shader = _workbench.Compile(name, form.Source, form.Profile), Feeder = feederShader };
        }
        catch (UnsupportedProgramException exception)
        {
            return failed with { Error = exception.Message };
        }
        catch (InvalidOperationException exception)
        {
            return failed with { Error = exception.Message.Split('\n')[0] };
        }
    }

    /// <summary>The program inside its stage harness, not compiled yet.</summary>
    private static PreparedForm Wrap(string name, ShaderStage stage, string rewritten)
    {
        switch (stage)
        {
            case ShaderStage.Pixel:
                var (pixel, targets) = StageHarness.Pixel(rewritten);

                return new PreparedForm(name, stage, pixel, "ps", [], null, targets, 0, null);
            case ShaderStage.Vertex:
                return new PreparedForm(name, stage, StageHarness.Vertex(rewritten), "cs", [], null, 0, StageHarness.VertexBytes(rewritten), null);
            case ShaderStage.Mesh:
                var (compute, _) = MeshComputeFallback.Rewrite(rewritten);

                return new PreparedForm(name, stage, compute, "cs", [], null, 0, 0, null);
            default:
                return new PreparedForm(name, stage, ComputeSource(rewritten), "cs", [], null, 0, 0, null);
        }
    }

    /// <summary>
    /// A compute program as it is, unless it has a group barrier: WARP's shader compiler crashes on a barrier inside the
    /// dispatch switch (and the group-wave variant, which would keep 4-lane waves from splitting the console wave, puts one
    /// in every wave operation).
    /// </summary>
    private static string ComputeSource(string hlsl)
    {
        if (hlsl.Contains("GroupMemoryBarrierWithGroupSync", StringComparison.Ordinal))
        {
            throw new NotSupportedException("group barrier in the dispatch switch crashes WARP's shader compiler");
        }

        return hlsl;
    }

    private static DifferentialOutcome CompileOutcome(string name, string stage, string? dispatch, string? structured)
    {
        if (dispatch is not null && structured is not null)
        {
            return new DifferentialOutcome(name, stage, DifferentialVerdict.Skipped, Cut($"build: {dispatch}"));
        }

        return new DifferentialOutcome(name, stage, DifferentialVerdict.Mismatch, Cut($"build: dispatch {dispatch ?? "ok"}; structured {structured ?? "ok"}"));
    }

    private (WarpResult? Result, string? Error) Launch(PreparedForm form, int seed)
    {
        var data = new SyntheticData(seed);

        try
        {
            return (Job(form, data), null);
        }
        catch (Exception exception) when (exception is InvalidOperationException or TimeoutException or SharpGen.Runtime.SharpGenException)
        {
            return (null, exception.GetType().Name + ": " + exception.Message.Split('\n')[0]);
        }
    }

    private WarpResult Job(PreparedForm form, SyntheticData data)
    {
        var runner = _fixture.Runner;

        switch (form.Stage)
        {
            case ShaderStage.Pixel:
                var framebuffer = Framebuffer.Fullscreen(form.Targets, TargetSize, TargetSize, isFloat: false);

                return runner.Graphics(new GraphicsJob(form.Feeder!, form.Shader, framebuffer, data.Bindings([])));
            case ShaderStage.Vertex:
                BufferResource[] output = [new(HeapSlots.VertexOutput, new uint[Math.Max(1, form.OutputBytes / 4)], Writable: true)];

                return runner.Compute(new ComputeJob(form.Shader, 1, 1, 1, data.Bindings(output)));
            case ShaderStage.Mesh:
                BufferResource[] fallback =
                [
                    new(HeapSlots.FallbackTable, new uint[MeshVertexBytes / 4], Writable: true),
                    new(HeapSlots.FallbackTable + 1, new uint[MeshIndexBytes / 4], Writable: true),
                    new(HeapSlots.FallbackTable + 2, [0, 1, 0, 0, 0, 0, 0, 0], Writable: true),
                    new(HeapSlots.FallbackTable + 3, new uint[VertexViewWords], Writable: false),
                ];

                return runner.Compute(new ComputeJob(form.Shader, 1, 1, 1, data.Bindings(fallback)) { FallbackTable = HeapSlots.FallbackTable });
            default:
                return runner.Compute(new ComputeJob(form.Shader, 1, 1, 1, data.Bindings([])));
        }
    }

    private static bool IsStall(string? error)
    {
        return error is not null && error.StartsWith(nameof(TimeoutException), StringComparison.Ordinal);
    }

    /// <summary>Why the structured form kept the dispatch loop, from its comment, or "identical".</summary>
    private static string FallbackReason(string hlsl)
    {
        const string Comment = "// control flow: dispatch loop (";

        var start = hlsl.IndexOf(Comment, StringComparison.Ordinal);

        if (start < 0)
        {
            return "identical";
        }

        var end = hlsl.IndexOf(')', start + Comment.Length);

        return hlsl[(start + Comment.Length)..end];
    }

    private static int StableSeed(string name)
    {
        var hash = 17;

        foreach (var character in name)
        {
            hash = unchecked((hash * 31) + character);
        }

        return hash;
    }

    private static string Cut(string text)
    {
        if (text.Length <= DetailLength)
        {
            return text;
        }

        return text[..DetailLength];
    }
}
