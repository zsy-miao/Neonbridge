// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Translation.Waves;

/// <summary>Runs statements on a <see cref="WindowState"/>; an effect that may differ from the console's is a reason to stop.</summary>
internal sealed class StatementTransfer
{
    private const string AllocationProbe = "WaveIsFirstLane";

    private readonly WindowState _state;
    private readonly int _threads;

    public StatementTransfer(WindowState state, int threads)
    {
        _state = state;
        _threads = threads;
    }

    /// <summary>Applies one statement; null, or why its effect may not be the console's.</summary>
    public string? Apply(WindowStatement statement)
    {
        if (statement.Kind is StatementKind.Effect or StatementKind.Other)
        {
            return Opaque(statement);
        }

        if (statement.Kind == StatementKind.Guarded)
        {
            AssignGuarded(statement.Target, statement.Value);
        }
        else if (statement.Kind == StatementKind.Assign && statement.IsMask)
        {
            AssignMask(statement.Target, statement.Value);
        }
        else if (statement.Kind == StatementKind.Assign)
        {
            AssignValue(statement.Target, statement.Value);
        }

        return null;
    }

    /// <summary>
    /// A lane mask. One inside an exact EXEC is exact when it is the console's on a window holding an active lane: on
    /// a window without one, both are empty there.
    /// </summary>
    private void AssignMask(string target, string value)
    {
        var kind = MaskAlgebra.KindOf(value, _state);
        var fact = ValueRules.Condition(value, _state);
        var active = _state.Get(WindowNames.Active);
        var full = value == "true" || (WindowNames.IsMask(value) && _state.Get(value).Full);
        var nonEmpty = full || (WindowNames.IsMask(value) && _state.Get(value).NonEmpty);
        var exact = fact.Exact;

        if (ValueFact.IsSubset(kind) && active.IsExact)
        {
            exact = fact.Held;
        }

        if (target == WindowNames.Active)
        {
            AssignExec(exact, nonEmpty, full, (active.Full || ValueFact.IsSubset(kind)) && exact == ValueFact.AllBits);

            return;
        }

        var uniform = Bits(MaskAlgebra.IsActiveUniform(value, _state));
        var allUniform = Bits(kind == MaskKind.Uniform);

        _state.Set(target, new ValueFact(exact, fact.Held, uniform, allUniform, kind, nonEmpty, null) { Full = full });
    }

    /// <summary>A new EXEC; on a window known to hold one of its lanes, what was exact given that is exact.</summary>
    private void AssignExec(uint exact, bool nonEmpty, bool full, bool shrinks)
    {
        _state.ExecChanging(shrinks);
        _state.Set(WindowNames.Active, ValueFact.Scalar with { Exact = exact, Held = exact, Kind = MaskKind.ActiveAnd, NonEmpty = nonEmpty, Full = full });

        if (nonEmpty && exact == ValueFact.AllBits)
        {
            _state.HoldsActiveLane();
        }
    }

    private void AssignValue(string target, string value)
    {
        var fact = ValueRules.Evaluate(value, _state, _threads);

        if (WindowNames.IsScalar(target))
        {
            fact = fact with { Uniform = ValueFact.AllBits, AllUniform = ValueFact.AllBits, Kind = MaskKind.Uniform };
        }

        _state.Set(target, fact);
    }

    /// <summary>
    /// <c>if (active) target = value</c>: active lanes take the value, the others keep theirs. A lane writes only when
    /// the window holds an active lane, so the value needs to be the console's only then.
    /// </summary>
    private void AssignGuarded(string target, string value)
    {
        var fact = ValueRules.Evaluate(value, _state, _threads);
        var old = _state.Get(target);
        var (exact, held) = (0u, 0u);

        if (_state.Get(WindowNames.Active).IsExact)
        {
            (exact, held) = (fact.Held & old.Exact, fact.Held & old.Held);
        }

        _state.Set(target, new ValueFact(exact, held, fact.Uniform, 0, MaskKind.Other, false, null));
    }

    /// <summary>
    /// A statement of another shape: an effect must read only console values (a guarded one only those it reads when
    /// the window holds an active lane); what it assigns is exact when everything read is.
    /// </summary>
    private string? Opaque(WindowStatement statement)
    {
        var calls = statement.Calls.Where(call => !statement.IsAllocation || call != AllocationProbe).ToList();
        var exact = ValueRules.IsExact(statement.Reads, calls, _state);
        var held = !calls.Any(call => ValueRules.IsWindowCall(call, _state)) && statement.Reads.All(name => _state.Get(name).IsHeld);
        var guardedHeld = statement.IsGuarded && _state.Get(WindowNames.Active).IsExact && held;

        if (statement.Kind == StatementKind.Effect && !exact && !guardedHeld)
        {
            return $"effect on values the window may not share with the console: {statement.Text}";
        }

        if (statement.IsAllocation)
        {
            _state.Allocated = true;
        }

        foreach (var target in statement.Assigned)
        {
            Assigned(target, Bits(exact), Bits(held));
        }

        return null;
    }

    private void Assigned(string target, uint exact, uint held)
    {
        if (target == WindowNames.Active)
        {
            _state.ExecChanging(false);
            _state.Set(target, ValueFact.Scalar with { Exact = exact, Held = exact, Kind = MaskKind.ActiveAnd });

            return;
        }

        if (WindowNames.IsScalar(target))
        {
            _state.Set(target, ValueFact.Scalar with { Exact = exact, Held = held });

            return;
        }

        _state.Set(target, ValueFact.Unknown with { Exact = exact, Held = held });
    }

    private static uint Bits(bool all)
    {
        if (all)
        {
            return ValueFact.AllBits;
        }

        return 0;
    }
}
