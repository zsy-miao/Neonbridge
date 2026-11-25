// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Cli.Setup;

/// <summary>
/// Runs the setup steps in order. A step whose fingerprint (with those of the steps it depends on) matches its record
/// and whose outputs exist is skipped; any other selected step runs and is recorded when it succeeds. A step that fails
/// stops the steps that depend on it; the others still run. A re-run after an interruption resumes at the first step
/// without a current record.
/// </summary>
internal sealed class SetupRunner
{
    private const string NoRecord = "none";

    private readonly IReadOnlyList<ISetupStep> _steps;
    private readonly StateStore _store;
    private readonly TextWriter _output;
    private readonly TimeProvider _time;
    private readonly Func<long> _freeBytes;

    /// <summary>Uses <paramref name="freeBytes"/> to read the free space of the work folder's drive.</summary>
    public SetupRunner(IReadOnlyList<ISetupStep> steps, StateStore store, TextWriter output, TimeProvider time, Func<long> freeBytes)
    {
        _steps = steps;
        _store = store;
        _output = output;
        _time = time;
        _freeBytes = freeBytes;
    }

    public IReadOnlyList<ISetupStep> Steps => _steps;

    /// <summary>The condition of every step now, in step order.</summary>
    public IReadOnlyList<StepCondition> Check(SetupContext context)
    {
        var conditions = new Dictionary<string, StepCondition>(StringComparer.Ordinal);
        var list = new List<StepCondition>();

        foreach (var step in _steps)
        {
            var condition = StepCondition.Waiting;

            if (step.DependsOn.All(name => conditions.GetValueOrDefault(name) == StepCondition.Done))
            {
                condition = Compare(step, context, Fingerprint(step, context));
            }

            conditions[step.Name] = condition;
            list.Add(condition);
        }

        return list;
    }

    /// <summary>The recorded state of a step, or null.</summary>
    public StepState? State(string step)
    {
        return _store.Read(step);
    }

    /// <summary>Runs the selected steps that are not done (all selected ones with <paramref name="force"/>); true when all finished.</summary>
    public bool Run(SetupContext context, IReadOnlySet<string>? only, bool force)
    {
        var plan = Check(context);
        var run = new SetupRun(only, force, _time.GetTimestamp(), Expected(plan, context, only, force));

        if (!RequirementsMet(plan, context, run))
        {
            return false;
        }

        var succeeded = true;

        for (var index = 0; index < _steps.Count; index++)
        {
            var label = $"[{index + 1}/{_steps.Count}] {_steps[index].Title}";

            if (!RunStep(_steps[index], label, context, run))
            {
                succeeded = false;
            }
        }

        return succeeded;
    }

    private bool RunStep(ISetupStep step, string label, SetupContext context, SetupRun run)
    {
        var blocker = step.DependsOn.FirstOrDefault(name => !run.Finished(name));

        if (!run.Selects(step.Name))
        {
            var current = StepCondition.Waiting;

            if (blocker is null)
            {
                current = Compare(step, context, Fingerprint(step, context));
            }

            if (current == StepCondition.Done)
            {
                run.Finish(step.Name);
            }

            _output.WriteLine($"{label}: not selected ({StepText.Describe(current)})");

            return true;
        }

        if (blocker is not null)
        {
            _output.WriteLine($"{label}: skipped, needs step {blocker} first");

            return false;
        }

        var fingerprint = Fingerprint(step, context);
        var condition = Compare(step, context, fingerprint);

        if (condition == StepCondition.Done && !run.Force)
        {
            _output.WriteLine($"{label}: up to date");
            run.Finish(step.Name);

            return true;
        }

        return HasSpace(step, label, context) && Execute(step, label, condition, fingerprint, context, run);
    }

    private bool Execute(ISetupStep step, string label, StepCondition condition, StepFingerprint fingerprint, SetupContext context, SetupRun run)
    {
        var expected = Expected(step, context);
        var elapsed = _time.GetElapsedTime(run.Started);
        var finish = _time.GetLocalNow() + run.Remaining;
        var reason = StepText.Describe(condition);

        if (condition == StepCondition.Done)
        {
            reason = "forced";
        }

        _output.WriteLine($"{label} ({reason}), about {StepText.Duration(expected)}");
        _output.WriteLine($"  overall: {StepText.Duration(elapsed)} elapsed, ~{StepText.Duration(run.Remaining)} left, finish ~{finish:HH:mm}");
        _output.Flush();

        _store.MarkStarted(step.Name);
        var started = _time.GetTimestamp();
        var result = Attempt(step, context);
        var duration = _time.GetElapsedTime(started);
        run.Remaining = Max(TimeSpan.Zero, run.Remaining - expected);

        if (!result.Succeeded)
        {
            _output.WriteLine($"{label} failed after {StepText.Duration(duration)}: {result.Summary}");

            return false;
        }

        _store.Write(new StepState(step.Name, fingerprint.Inputs, fingerprint.Tool, _time.GetUtcNow(), duration, result.Summary));
        run.Finish(step.Name);
        _output.WriteLine($"{label} done in {StepText.Duration(duration)}: {result.Summary}");

        return true;
    }

    private static StepResult Attempt(ISetupStep step, SetupContext context)
    {
        try
        {
            return step.Run(context);
        }
        catch (Exception exception) when (exception is not OutOfMemoryException)
        {
            return StepResult.Failed($"{exception.GetType().Name}: {exception.Message}");
        }
    }

    private bool HasSpace(ISetupStep step, string label, SetupContext context)
    {
        var required = step.RequiredBytes(context);

        if (required <= 0)
        {
            return true;
        }

        var free = _freeBytes();

        if (required <= free)
        {
            return true;
        }

        _output.WriteLine($"{label}: needs about {StepText.Size(required)} free in {context.Work}, {StepText.Size(free)} left;");
        _output.WriteLine("  free some space or pick another folder with --work <dir>");

        return false;
    }

    /// <summary>Prints every tool a step to run is missing; true when none is.</summary>
    private bool RequirementsMet(IReadOnlyList<StepCondition> plan, SetupContext context, SetupRun run)
    {
        var missing = new List<string>();

        for (var index = 0; index < _steps.Count; index++)
        {
            var step = _steps[index];
            var message = step.MissingRequirement(context);

            if (message is not null && run.Selects(step.Name) && (plan[index] != StepCondition.Done || run.Force))
            {
                missing.Add($"  {step.Name}: {message}");
            }
        }

        if (missing.Count == 0)
        {
            return true;
        }

        _output.WriteLine("setup cannot start; missing requirements:");
        missing.ForEach(_output.WriteLine);

        return false;
    }

    /// <summary>The step's own fingerprint, its inputs combined with the records of the steps it depends on.</summary>
    private StepFingerprint Fingerprint(ISetupStep step, SetupContext context)
    {
        var own = step.Fingerprint(context);
        var parts = new List<string> { own.Inputs };

        foreach (var name in step.DependsOn)
        {
            var state = _store.Read(name);
            parts.Add(state?.Inputs ?? NoRecord);
            parts.Add(state?.Tool ?? NoRecord);
        }

        return own with { Inputs = Fingerprints.Combine([.. parts]) };
    }

    private StepCondition Compare(ISetupStep step, SetupContext context, StepFingerprint fingerprint)
    {
        var state = _store.Read(step.Name);

        if (_store.WasInterrupted(step.Name))
        {
            return StepCondition.Interrupted;
        }

        if (state is null)
        {
            return StepCondition.NotRun;
        }

        if (state.Tool != fingerprint.Tool)
        {
            return StepCondition.ToolChanged;
        }

        if (state.Inputs != fingerprint.Inputs)
        {
            return StepCondition.InputsChanged;
        }

        if (!step.HasOutputs(context))
        {
            return StepCondition.OutputsMissing;
        }

        return StepCondition.Done;
    }

    private TimeSpan Expected(IReadOnlyList<StepCondition> plan, SetupContext context, IReadOnlySet<string>? only, bool force)
    {
        var total = TimeSpan.Zero;

        for (var index = 0; index < _steps.Count; index++)
        {
            var selected = only is null || only.Contains(_steps[index].Name);

            if (selected && (plan[index] != StepCondition.Done || force))
            {
                total += Expected(_steps[index], context);
            }
        }

        return total;
    }

    /// <summary>The last measured time of the step, or its estimate.</summary>
    private TimeSpan Expected(ISetupStep step, SetupContext context)
    {
        var state = _store.Read(step.Name);

        if (state is null)
        {
            return step.Estimate(context);
        }

        return state.Duration;
    }

    private static TimeSpan Max(TimeSpan first, TimeSpan second)
    {
        if (first > second)
        {
            return first;
        }

        return second;
    }
}
