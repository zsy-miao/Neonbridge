// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using System.Numerics;
using System.Text.RegularExpressions;
using FiveStar.Shaders.Translation.Core;

namespace FiveStar.Shaders.Translation.Waves;

/// <summary>
/// The floor of a pixel or vertex program below its widest lane reach. A hardware wave of H lanes narrower than the
/// console wave holds program lanes 0 .. H-1; the others are missing. On the console those lanes exist but hold no pixel
/// (or vertex): their EXEC bit is clear, and in whole-wave mode (EXEC set to all lanes) they compute what the program
/// gives them, typically the identity of a reduction (0 for a sum, all ones for a minimum). The program gives console
/// results on H lanes when every read that may reach lane H or above reads a value known on such a lane at that point:
/// the read then returns that value for a missing lane (<c>vw_read_fill</c>, see <see cref="Rewrite"/>).
/// A forward pass tracks per variable its <see cref="MissingValue"/> on a lane without a pixel; a read by that lane is
/// known only when it stays inside lanes H and above. A store such a lane may make fails the check.
/// </summary>
internal sealed partial class MissingLaneAnalysis
{
    private const int StepsPerBlock = 64;
    private const string Active = "active";
    private const string Fill = "vw_read_fill(";

    // vw_read(x, lane)
    private const int ReadArguments = 2;

    // The prologue's statements are keyed below every block address.
    private const int PrologueAddress = int.MinValue;

    private readonly WindowGraph _graph;
    private readonly IReadOnlyList<WindowStatement> _prologue;
    private readonly int _wave;
    private readonly Dictionary<string, string> _temporaries = new(StringComparer.Ordinal);

    // Per read that may reach a missing lane (block, statement, read in the statement): the value it reads there.
    private Dictionary<(int Block, int Statement, int Read), uint> _fills = [];

    public MissingLaneAnalysis(IReadOnlyList<CodeBlock> blocks, IReadOnlyList<string> prologue, int wave)
    {
        _graph = new WindowGraph(blocks);
        _prologue = prologue.SelectMany(line => line.Split('\n')).Select(WindowStatement.Parse).ToList();
        _wave = wave;

        // Temporaries are assigned once: a lane number held in one is read from its definition.
        foreach (var statement in _graph.Blocks.SelectMany(block => block.Statements))
        {
            if (statement.Kind == StatementKind.Assign && TemporaryPattern().IsMatch(statement.Target))
            {
                _temporaries[statement.Target] = statement.Value;
            }
        }
    }

    /// <summary>After <see cref="Floor"/>: why the next narrower wave differs, or null.</summary>
    public string? Reason { get; private set; }

    /// <summary>
    /// The narrowest hardware wave (a power of two from 4) below <paramref name="reach"/> with console results, or the
    /// reach; every wave between it and the reach is checked too, with the same value for each read.
    /// </summary>
    public int Floor(int reach)
    {
        var floor = reach;
        var fills = new Dictionary<(int Block, int Statement, int Read), uint>();

        for (var lanes = reach >> 1; lanes >= WaveOps.NarrowestHardwareWave; lanes >>= 1)
        {
            _fills = new Dictionary<(int Block, int Statement, int Read), uint>(fills);
            Reason = Check(lanes);

            if (Reason is not null)
            {
                break;
            }

            fills = _fills;
            floor = lanes;
        }

        _fills = fills;

        return floor;
    }

    /// <summary>Makes each read that may reach a missing lane return the value that lane holds (only those not 0 change).</summary>
    public void Rewrite(IReadOnlyList<CodeBlock> blocks)
    {
        foreach (var block in blocks)
        {
            var statement = 0;

            for (var line = 0; line < block.Lines.Count; line++)
            {
                var parts = block.Lines[line].Split('\n');

                for (var part = 0; part < parts.Length; part++, statement++)
                {
                    parts[part] = WithFills(parts[part], block.Address, statement);
                }

                block.Lines[line] = string.Join('\n', parts);
            }
        }
    }

    private string WithFills(string text, int block, int statement)
    {
        var calls = ReadPattern().Matches(text);

        for (var read = calls.Count - 1; read >= 0; read--)
        {
            if (!_fills.TryGetValue((block, statement, read), out var fill) || fill == 0)
            {
                continue;
            }

            var start = calls[read].Index + calls[read].Length;
            var close = Closing(text, start);
            var value = string.Create(CultureInfo.InvariantCulture, $", 0x{fill:x}u");
            text = text[..calls[read].Index] + Fill + text[start..close] + value + text[close..];
        }

        return text;
    }

    /// <summary>Null when hardware waves of <paramref name="lanes"/> lanes give console results, else the first reason why not.</summary>
    public string? Check(int lanes)
    {
        var classes = BitOperations.Log2((uint)_wave) - BitOperations.Log2((uint)lanes);
        var start = Enumerable.Range(0, classes)
            .Select(_ => new Dictionary<string, MissingValue>(StringComparer.Ordinal) { [Active] = MissingValue.Bool(false) })
            .ToArray();
        var reason = Run(PrologueAddress, _prologue, start, lanes);

        if (reason is not null)
        {
            return reason;
        }

        var entries = new Dictionary<int, Dictionary<string, MissingValue>[]> { [_graph.Entry] = start };
        var pending = new Queue<int>([_graph.Entry]);
        var limit = StepsPerBlock * (_graph.Blocks.Count() + 1);

        for (var step = 0; pending.Count > 0; step++)
        {
            var block = _graph.Find(pending.Dequeue());

            if (step > limit)
            {
                return "the analysis did not settle";
            }

            if (block is null)
            {
                continue;
            }

            var states = entries[block.Address].Select(state => new Dictionary<string, MissingValue>(state, StringComparer.Ordinal)).ToArray();
            reason = Run(block.Address, block.Statements, states, lanes);

            if (reason is not null)
            {
                return reason;
            }

            // Branch conditions are wave-uniform: the lanes follow every path their wave may take.
            foreach (var successor in block.Successors)
            {
                Propagate(entries, pending, successor, states);
            }
        }

        return null;
    }

    private string? Run(int address, IReadOnlyList<WindowStatement> statements, Dictionary<string, MissingValue>[] states, int lanes)
    {
        for (var index = 0; index < statements.Count; index++)
        {
            var reason = Apply(statements[index], (address, index), states, lanes);

            if (reason is not null)
            {
                return reason;
            }
        }

        return null;
    }

    private static void Propagate(Dictionary<int, Dictionary<string, MissingValue>[]> entries, Queue<int> pending, int successor, Dictionary<string, MissingValue>[] states)
    {
        if (!entries.TryGetValue(successor, out var known))
        {
            entries[successor] = states.Select(state => new Dictionary<string, MissingValue>(state, StringComparer.Ordinal)).ToArray();
            pending.Enqueue(successor);

            return;
        }

        var changed = false;

        for (var index = 0; index < states.Length; index++)
        {
            // A variable absent from a state is unknown, so the merge keeps what both agree on.
            var state = states[index];
            var lost = known[index].Where(entry => Get(state, entry.Key) != entry.Value).Select(entry => entry.Key).ToList();
            lost.ForEach(name => known[index].Remove(name));
            changed |= lost.Count > 0;
        }

        if (changed)
        {
            pending.Enqueue(successor);
        }
    }

    /// <summary>Applies one statement on the lanes without a pixel; why the floor breaks there, or null.</summary>
    private string? Apply(WindowStatement statement, (int Block, int Statement) key, Dictionary<string, MissingValue>[] states, int lanes)
    {
        if (statement.Kind == StatementKind.Control)
        {
            return null;
        }

        var reason = Reads(statement.Text, key, states, lanes);

        if (reason is not null)
        {
            return reason;
        }

        // Every class reads the others as they were before the statement.
        var assigned = Enumerable.Range(0, states.Length).Select(index => Assigned(statement, states, index, lanes)).ToList();

        for (var index = 0; index < states.Length; index++)
        {
            reason = Commit(statement, states[index], assigned[index]);

            if (reason is not null)
            {
                return reason;
            }
        }

        return null;
    }

    private static string? Commit(WindowStatement statement, Dictionary<string, MissingValue> state, MissingValue assigned)
    {
        var active = Get(state, Active);

        switch (statement.Kind)
        {
            case StatementKind.Assign:
            case StatementKind.Guarded when active == MissingValue.Bool(true):
                Set(state, statement.Target, assigned);

                return null;
            case StatementKind.Guarded when active == MissingValue.Bool(false):
                return null;
            case StatementKind.Guarded:
                Set(state, statement.Target, MissingValue.Merge(Get(state, statement.Target), assigned));

                return null;
            default:
                statement.Assigned.ToList().ForEach(name => state.Remove(name));

                return Store(statement, active);
        }
    }

    /// <summary>The value an assignment writes, converted to its target's type (a lane mask is a bool, a register a uint).</summary>
    private MissingValue Assigned(WindowStatement statement, Dictionary<string, MissingValue>[] states, int index, int lanes)
    {
        if (statement.Kind is not (StatementKind.Assign or StatementKind.Guarded))
        {
            return MissingValue.Unknown;
        }

        var value = Evaluate(statement.Value, states, index, lanes);

        if (value.Kind == MissingKind.MaskLane)
        {
            return value;
        }

        if (statement.IsMask)
        {
            return MissingExpression.Convert(value, MissingType.Bool);
        }

        return MissingExpression.Convert(value, MissingType.UInt);
    }

    /// <summary>The console lane stores too when its EXEC bit may be set; the hardware wave has no such lane.</summary>
    private static string? Store(WindowStatement statement, MissingValue active)
    {
        if (statement.IsShared && active != MissingValue.Bool(false))
        {
            return $"a store a lane without a pixel may make: {statement.Text}";
        }

        return null;
    }

    /// <summary>An expression on the lanes of class <paramref name="index"/> (lanes H·2^index .. H·2^(index+1)-1).</summary>
    private MissingValue Evaluate(string value, Dictionary<string, MissingValue>[] states, int index, int lanes)
    {
        if (value.Contains("firstbitlow(", StringComparison.Ordinal) && IsBallot(value))
        {
            return MissingValue.MaskLane;
        }

        return MissingExpression.Evaluate(value, name => Variable(states[index], name), (x, lane) => ReadByMissing(x, lane, states, index, lanes));
    }

    /// <summary>A variable's value; a temporary holding a lane number of an aligned group is below the wave size.</summary>
    private MissingValue Variable(Dictionary<string, MissingValue> state, string name)
    {
        var value = Get(state, name);

        if (value.Kind != MissingKind.Unknown || !_temporaries.ContainsKey(name))
        {
            return value;
        }

        var text = Resolve(name);

        if (AlignedPattern().IsMatch(text) || SwapPattern().IsMatch(text))
        {
            return new MissingValue(MissingKind.LaneNumber, (uint)_wave, MissingType.UInt);
        }

        return value;
    }

    /// <summary>The first set lane of a ballot (s_ff1 of a lane mask), directly or through the temporary holding the ballot.</summary>
    private bool IsBallot(string value)
    {
        if (value.Contains("vw_ballot(", StringComparison.Ordinal))
        {
            return true;
        }

        return WindowStatement.Names(value).Any(name => _temporaries.TryGetValue(name, out var definition) && definition.StartsWith("vw_ballot(", StringComparison.Ordinal));
    }

    /// <summary>
    /// What a lane of class <paramref name="index"/> reads: known when the lane it reads has no pixel either. A class
    /// starts at a multiple of its size, so an aligned group no larger stays inside it; a larger one reaches lane 0.
    /// </summary>
    private MissingValue ReadByMissing(string x, string lane, Dictionary<string, MissingValue>[] states, int index, int lanes)
    {
        var (group, fixedLane) = Target(lane, states[index]);

        if (fixedLane is not null && fixedLane.Value < lanes)
        {
            return MissingValue.Unknown;
        }

        if (fixedLane is not null)
        {
            return MissingExpression.Convert(Evaluate(x, states, ClassOf(fixedLane.Value, lanes), lanes), MissingType.UInt);
        }

        if (group > lanes << index)
        {
            return MissingValue.Unknown;
        }

        return MissingExpression.Convert(Evaluate(x, states, index, lanes), MissingType.UInt);
    }

    /// <summary>
    /// Every <c>vw_read</c> of the statement by a lane with a pixel that may reach lane H or above reads one known value
    /// there, the same at every visit; it is recorded as the read's fill.
    /// </summary>
    private string? Reads(string text, (int Block, int Statement) key, Dictionary<string, MissingValue>[] states, int lanes)
    {
        var calls = ReadPattern().Matches(text);

        for (var read = 0; read < calls.Count; read++)
        {
            var arguments = Arguments(text, calls[read].Index + calls[read].Length);

            if (arguments is null)
            {
                return $"a read not understood: {text}";
            }

            var values = Reached(arguments.Value.Lane, states, lanes)
                .Select(index => MissingExpression.Convert(Evaluate(arguments.Value.X, states, index, lanes), MissingType.UInt))
                .Distinct()
                .ToList();

            if (values.Count == 0)
            {
                continue;
            }

            var id = (key.Block, key.Statement, read);

            if (values.Count > 1 || !values[0].IsKnown || (_fills.TryGetValue(id, out var fill) && fill != values[0].Bits))
            {
                return $"a read of lane {lanes} or above, unknown there: {text}";
            }

            _fills[id] = values[0].Bits;
        }

        return null;
    }

    /// <summary>The classes of missing lanes a read by a lane below H may reach.</summary>
    private IEnumerable<int> Reached(string lane, Dictionary<string, MissingValue>[] states, int lanes)
    {
        // Lane numbers come from scalar registers, equal in every class.
        var (group, fixedLane) = Target(lane, states[0]);

        if (fixedLane is not null && fixedLane.Value >= lanes)
        {
            return [ClassOf(fixedLane.Value, lanes)];
        }

        if (fixedLane is not null)
        {
            return [];
        }

        return Enumerable.Range(0, states.Length).Where(index => lanes << index < group);
    }

    /// <summary>The class of a lane at H or above: the power of two its number has over H.</summary>
    private static int ClassOf(long lane, int lanes)
    {
        return BitOperations.Log2((uint)lane) - BitOperations.Log2((uint)lanes);
    }

    /// <summary>
    /// The aligned lane group a read stays in, or the fixed lane it reads: <c>(vw_lane() &amp; ~m) | ...</c> stays in
    /// groups of m + 1, <c>vw_lane() ^ k</c> in the power of two above k, a lane taken from a lane mask in its own; any
    /// other lane may be anywhere in the wave.
    /// </summary>
    private (int Group, long? Fixed) Target(string lane, Dictionary<string, MissingValue> state)
    {
        var text = Resolve(lane);
        var number = NumberPattern().Match(text);

        if (number.Success && number.Groups["hex"].Success)
        {
            return (_wave, long.Parse(number.Groups["hex"].Value, NumberStyles.HexNumber, CultureInfo.InvariantCulture) & (_wave - 1));
        }

        if (number.Success)
        {
            return (_wave, long.Parse(number.Groups["dec"].Value, CultureInfo.InvariantCulture) & (_wave - 1));
        }

        var aligned = AlignedPattern().Match(text);

        if (aligned.Success)
        {
            return (int.Parse(aligned.Groups["mask"].Value, CultureInfo.InvariantCulture) + 1, null);
        }

        var swap = SwapPattern().Match(text);

        if (swap.Success)
        {
            return ((int)BitOperations.RoundUpToPowerOf2(uint.Parse(swap.Groups["bits"].Value, CultureInfo.InvariantCulture) + 1), null);
        }

        if (Get(state, text).Kind == MissingKind.MaskLane)
        {
            return (1, null);
        }

        return (_wave, null);
    }

    /// <summary>A lane expression without enclosing parentheses, the guard of an invalid lane and temporaries.</summary>
    private string Resolve(string lane)
    {
        var text = BoolSyntax.Unwrap(lane);

        for (var depth = 0; depth < StepsPerBlock; depth++)
        {
            var guarded = GuardedLanePattern().Match(text);

            if (guarded.Success)
            {
                text = guarded.Groups["lane"].Value;
            }

            if (!_temporaries.TryGetValue(text, out var definition))
            {
                return text;
            }

            text = BoolSyntax.Unwrap(definition);
        }

        return text;
    }

    /// <summary>The two arguments of a call whose opening parenthesis ends at <paramref name="start"/>.</summary>
    private static (string X, string Lane)? Arguments(string text, int start)
    {
        var close = Closing(text, start);

        if (close < 0)
        {
            return null;
        }

        var parts = BoolSyntax.Split(text[start..close], ",");

        if (parts.Count != ReadArguments)
        {
            return null;
        }

        return (parts[0], parts[1]);
    }

    /// <summary>The index of the parenthesis closing a call whose opening parenthesis ends at <paramref name="start"/>, or -1.</summary>
    private static int Closing(string text, int start)
    {
        var depth = 1;

        for (var index = start; index < text.Length; index++)
        {
            if (text[index] == '(')
            {
                depth++;
            }
            else if (text[index] == ')')
            {
                depth--;
            }

            if (depth == 0)
            {
                return index;
            }
        }

        return -1;
    }

    private static MissingValue Get(Dictionary<string, MissingValue> state, string name)
    {
        return state.GetValueOrDefault(name, MissingValue.Unknown);
    }

    private static void Set(Dictionary<string, MissingValue> state, string name, MissingValue value)
    {
        if (value.Kind == MissingKind.Unknown)
        {
            state.Remove(name);

            return;
        }

        state[name] = value;
    }

    [GeneratedRegex(@"^_t\d+$")]
    private static partial Regex TemporaryPattern();

    [GeneratedRegex(@"\bvw_read\(")]
    private static partial Regex ReadPattern();

    [GeneratedRegex(@"^(?:0x(?<hex>[0-9a-fA-F]+)|(?<dec>\d+))u?$")]
    private static partial Regex NumberPattern();

    [GeneratedRegex(@"^\(*vw_lane\(\) & ~(?<mask>\d+)u\)")]
    private static partial Regex AlignedPattern();

    [GeneratedRegex(@"^\(*vw_lane\(\) \^ (?<bits>\d+)u\)*$")]
    private static partial Regex SwapPattern();

    [GeneratedRegex(@"^(?<lane>\w+) == 0xffffffffu \? 0u : \k<lane>$")]
    private static partial Regex GuardedLanePattern();
}
