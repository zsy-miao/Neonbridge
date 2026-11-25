// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Collections;
using System.Globalization;
using System.Text.RegularExpressions;
using FiveStar.Shaders.Translation.Core;

namespace FiveStar.Shaders.Translation.Waves;

/// <summary>
/// Liveness-aware carry for the superstep form (<see cref="TranslationOptions.SuperstepCarry"/>). Its dispatch switch has
/// thousands of cases, and every register live into any of them made the compiler join it after the switch and at the
/// loop header: a phi per register with an operand per case. Here registers pass between cases through carry slots
/// instead, one slot shared by registers never live at the same case boundary: a case loads the slots of the registers
/// it reads before writing and stores the registers it touches that a later case reads. Every register is then dead at
/// the loop header, so only the slots are joined there. A register no case on any path from the entry has mentioned
/// still holds its declared zero, so it is carried only once one may have: a VGPR first written under EXEC
/// (<c>if (active) v = ..</c>) reads its old value and was live from the entry to there. A case that reads one before
/// then sets it to zero instead of loading it.
/// </summary>
internal sealed partial class SuperstepCarry
{
    private const string HexPrefix = "0x";

    private readonly List<string> _names;
    private readonly Dictionary<string, int> _index;
    private readonly Dictionary<string, string> _types;
    private readonly Dictionary<int, string> _slots = [];
    private readonly List<string> _declarations = [];
    private readonly List<string> _entry = [];

    /// <param name="registers">The function-scope registers and their HLSL types.</param>
    /// <param name="pinned">Registers read after the dispatch loop: they stay as they are.</param>
    private SuperstepCarry(IReadOnlyDictionary<string, string> registers, IReadOnlySet<string> pinned)
    {
        _names = [.. registers.Keys.Where(name => !pinned.Contains(name)).Order(StringComparer.Ordinal)];
        _index = _names.Select((name, index) => (name, index)).ToDictionary(pair => pair.name, pair => pair.index);
        _types = registers.ToDictionary(pair => pair.Key, pair => pair.Value);
    }

    /// <summary>Slot declarations, before the loop.</summary>
    public IReadOnlyList<string> Declarations => _declarations;

    /// <summary>Stores of the registers the first case reads, after the prologue.</summary>
    public IReadOnlyList<string> Entry => _entry;

    /// <summary>The blocks with their loads and stores.</summary>
    public List<CodeBlock> Blocks { get; } = [];

    /// <summary>The number of registers that pass through slots and the number of slots.</summary>
    public (int Registers, int Slots) Size { get; private set; }

    /// <summary>The most registers carried across one stop (a wave operation or a barrier).</summary>
    public int MostAtStop { get; private set; }

    /// <summary>
    /// The carry of <paramref name="blocks"/>; the first case is the block at address 0. <paramref name="before"/> are the
    /// lines that run before the loop (the registers they mention may no longer hold their declared values).
    /// </summary>
    public static SuperstepCarry Plan(
        IReadOnlyList<CodeBlock> blocks, IReadOnlyDictionary<string, string> registers, IReadOnlySet<string> pinned, IEnumerable<string> before)
    {
        var carry = new SuperstepCarry(registers, pinned);
        carry.Build(blocks, before);

        return carry;
    }

    private void Build(IReadOnlyList<CodeBlock> blocks, IEnumerable<string> before)
    {
        var count = _names.Count;
        var facts = blocks.Select(block => Scan(block.Lines, count)).ToList();
        var successors = Successors(blocks);
        var (liveIn, liveOut) = Liveness(facts, successors, count);
        var entry = blocks.ToList().FindIndex(block => block.Address == 0);
        var touchedIn = Touched(facts, successors, entry, Initial(before), count);

        // A boundary carries the registers live there that a case after it may find touched; the others are zero.
        var carriedOut = liveOut.Select((live, block) => Carried(live, successors[block], touchedIn)).ToList();
        var entryCarried = new BitArray(count);

        if (entry >= 0)
        {
            entryCarried.Or(liveIn[entry]).And(touchedIn[entry]);
        }

        var slots = Color([.. carriedOut, entryCarried]);
        _entry.AddRange(Members(entryCarried).Select(register => $"  {Slot(slots[register])} = {_names[register]};"));
        MostAtStop = Enumerable.Range(0, blocks.Count).Where(block => IsStop(blocks[block])).Select(block => Members(carriedOut[block]).Count).DefaultIfEmpty(0).Max();

        for (var block = 0; block < blocks.Count; block++)
        {
            var lines = Loads(liveIn[block], facts[block].Touched, touchedIn[block], slots);
            lines.AddRange(blocks[block].Lines);
            lines.AddRange(Stores(carriedOut[block], facts[block].Touched, touchedIn[block], slots));
            Blocks.Add(blocks[block] with { Lines = lines });
        }
    }

    /// <summary>A case's loads of the registers it reads before writing: from their slots, or zero if none was touched yet.</summary>
    private List<string> Loads(BitArray liveIn, BitArray touched, BitArray touchedIn, int[] slots)
    {
        var read = new BitArray(liveIn).And(touched);
        var lines = Members(new BitArray(read).And(touchedIn)).Select(register => $"  {_names[register]} = {Slot(slots[register])};").ToList();
        lines.AddRange(Members(read.And(new BitArray(touchedIn).Not())).Select(register => $"  {_names[register]} = {Zero(register)};"));

        return lines;
    }

    /// <summary>
    /// A case's stores of the registers carried out of it: those it touched, and zero for those no case had touched yet
    /// (a successor reached on another path may find them touched); the others are in their slots already.
    /// </summary>
    private List<string> Stores(BitArray carriedOut, BitArray touched, BitArray touchedIn, int[] slots)
    {
        var lines = Members(new BitArray(carriedOut).And(touched)).Select(register => $"  {Slot(slots[register])} = {_names[register]};").ToList();
        var zero = new BitArray(carriedOut).And(new BitArray(touched).Not()).And(new BitArray(touchedIn).Not());
        lines.AddRange(Members(zero).Select(register => $"  {Slot(slots[register])} = {Zero(register)};"));

        return lines;
    }

    /// <summary>The registers live into and out of each block, backward to a fixed point.</summary>
    private static (List<BitArray> LiveIn, List<BitArray> LiveOut) Liveness(
        List<(BitArray Used, BitArray Defined, BitArray Touched)> facts, List<List<int>> successors, int count)
    {
        var liveIn = facts.Select(_ => new BitArray(count)).ToList();
        var liveOut = facts.Select(_ => new BitArray(count)).ToList();

        // The blocks are in address order, so walking them in reverse converges quickly.
        for (var changed = true; changed;)
        {
            changed = false;

            for (var block = facts.Count - 1; block >= 0; block--)
            {
                var output = new BitArray(count);
                successors[block].ForEach(next => output.Or(liveIn[next]));
                var input = new BitArray(output).And(new BitArray(facts[block].Defined).Not()).Or(facts[block].Used);
                changed |= !Same(input, liveIn[block]);
                liveOut[block] = output;
                liveIn[block] = input;
            }
        }

        return (liveIn, liveOut);
    }

    /// <summary>
    /// Per block, the registers a case on some path from the entry to it may have touched (forward to a fixed point);
    /// the others still hold their declared zero there. A block no path reaches gets every register.
    /// </summary>
    private static List<BitArray> Touched(
        List<(BitArray Used, BitArray Defined, BitArray Touched)> facts, List<List<int>> successors, int entry, BitArray initial, int count)
    {
        var reached = facts.Select(_ => false).ToArray();
        var touchedIn = facts.Select(_ => new BitArray(count)).ToList();

        if (entry >= 0)
        {
            touchedIn[entry].Or(initial);
            reached[entry] = true;
        }

        for (var changed = true; changed;)
        {
            changed = false;

            for (var block = 0; block < facts.Count; block++)
            {
                if (!reached[block])
                {
                    continue;
                }

                var output = new BitArray(touchedIn[block]).Or(facts[block].Touched);

                foreach (var next in successors[block])
                {
                    var merged = new BitArray(touchedIn[next]).Or(output);
                    changed |= !reached[next] || !Same(merged, touchedIn[next]);
                    reached[next] = true;
                    touchedIn[next] = merged;
                }
            }
        }

        for (var block = 0; block < facts.Count; block++)
        {
            if (!reached[block])
            {
                touchedIn[block].SetAll(true);
            }
        }

        return touchedIn;
    }

    /// <summary>The registers the lines before the loop mention, and <c>active</c> (declared true): not known to be zero.</summary>
    private BitArray Initial(IEnumerable<string> before)
    {
        var initial = new BitArray(_names.Count);

        foreach (var register in before.SelectMany(Mentions).Append(_index.GetValueOrDefault(WindowNames.Active, -1)).Where(register => register >= 0))
        {
            initial[register] = true;
        }

        return initial;
    }

    /// <summary>The registers live out of a block that a successor may find touched.</summary>
    private static BitArray Carried(BitArray liveOut, List<int> successors, List<BitArray> touchedIn)
    {
        var touched = new BitArray(liveOut.Length);
        successors.ForEach(next => touched.Or(touchedIn[next]));

        return touched.And(liveOut);
    }

    private static bool IsStop(CodeBlock block)
    {
        return block.Lines.Any(line => line.Contains("pc = PC_WAVE", StringComparison.Ordinal) || line.Contains("pc = PC_YIELD", StringComparison.Ordinal));
    }

    /// <summary>The declared value of a register: zero of its type.</summary>
    private string Zero(int register)
    {
        var type = _types[_names[register]];

        return $"({type})0";
    }

    /// <summary>Upward-exposed reads, certain writes and every mention of the carried registers in one block.</summary>
    private (BitArray Used, BitArray Defined, BitArray Touched) Scan(IReadOnlyList<string> lines, int count)
    {
        var used = new BitArray(count);
        var defined = new BitArray(count);
        var touched = new BitArray(count);
        var depth = 0;

        foreach (var raw in lines)
        {
            var comment = raw.IndexOf("//", StringComparison.Ordinal);
            var line = comment < 0 ? raw : raw[..comment];
            var write = depth == 0 && !Stages.MeshDeclarations.WritesOutput(line) ? WritePattern().Match(line) : Match.Empty;
            var reads = line;

            if (write.Success)
            {
                reads = write.Groups["value"].Value;
            }

            foreach (var register in Mentions(reads))
            {
                touched[register] = true;
                used[register] = used[register] || !defined[register];
            }

            if (write.Success && _index.TryGetValue(write.Groups["name"].Value, out var target))
            {
                touched[target] = true;
                defined[target] = true;
            }

            depth += line.Count(character => character == '{') - line.Count(character => character == '}');
        }

        return (used, defined, touched);
    }

    private IEnumerable<int> Mentions(string text)
    {
        foreach (Match name in NamePattern().Matches(text))
        {
            if (_index.TryGetValue(name.Value, out var register))
            {
                yield return register;
            }
        }
    }

    /// <summary>The cases each block's <c>pc</c> and <c>pc_resume</c> assignments reach; every case for one it cannot read.</summary>
    private static List<List<int>> Successors(IReadOnlyList<CodeBlock> blocks)
    {
        var nodes = blocks.Select((block, index) => (block.Address, index)).ToDictionary(pair => (long)pair.Address, pair => pair.index);
        var all = Enumerable.Range(0, blocks.Count).ToList();
        var result = new List<List<int>>();

        foreach (var block in blocks)
        {
            var targets = new List<int>();

            foreach (Match assignment in JumpPattern().Matches(string.Join("\n", block.Lines)))
            {
                // The condition of a branch is a value; its targets are not.
                var value = assignment.Groups["value"].Value;
                value = value[(value.LastIndexOf('?') + 1)..];

                if (OpaqueTargetPattern().IsMatch(value))
                {
                    targets = all;

                    break;
                }

                foreach (Match address in AddressPattern().Matches(value))
                {
                    if (nodes.TryGetValue(long.Parse(address.Value[HexPrefix.Length..], NumberStyles.HexNumber, CultureInfo.InvariantCulture), out var node))
                    {
                        targets.Add(node);
                    }
                }
            }

            result.Add(targets);
        }

        return result;
    }

    /// <summary>Greedy slots per type: registers live out of the same case (or into the first) get different ones.</summary>
    private int[] Color(IReadOnlyList<BitArray> boundaries)
    {
        var count = _names.Count;
        var neighbours = Enumerable.Range(0, count).Select(_ => new BitArray(count)).ToArray();
        var carried = new BitArray(count);

        foreach (var boundary in boundaries)
        {
            carried.Or(boundary);
            Members(boundary).ForEach(register => neighbours[register].Or(boundary));
        }

        var slots = Enumerable.Repeat(-1, count).ToArray();
        var perType = new Dictionary<string, List<int>>();

        foreach (var register in Members(carried))
        {
            var type = _types[_names[register]];
            var taken = Members(neighbours[register]).Where(other => slots[other] >= 0).Select(other => slots[other]).ToHashSet();

            if (!perType.TryGetValue(type, out var pool))
            {
                pool = [];
                perType[type] = pool;
            }

            var slot = pool.FirstOrDefault(candidate => !taken.Contains(candidate), -1);

            if (slot < 0)
            {
                slot = _slots.Count;
                _slots[slot] = $"ss_c{slot}";
                pool.Add(slot);
                _declarations.Add($"  {type} {_slots[slot]} = ({type})0;");
            }

            slots[register] = slot;
        }

        Size = (Members(carried).Count, _slots.Count);

        return slots;
    }

    private string Slot(int slot)
    {
        return _slots[slot];
    }

    private static List<int> Members(BitArray bits)
    {
        var members = new List<int>();

        for (var index = 0; index < bits.Length; index++)
        {
            if (bits[index])
            {
                members.Add(index);
            }
        }

        return members;
    }

    private static bool Same(BitArray left, BitArray right)
    {
        return !new BitArray(left).Xor(right).Cast<bool>().Any(bit => bit);
    }

    // A whole-line write of one variable, outside any braces of the block: the variable is certainly written.
    [GeneratedRegex(@"^\s*(?<name>[A-Za-z_]\w*)\s*=(?!=)\s*(?<value>[^;{}]*);\s*$")]
    private static partial Regex WritePattern();

    [GeneratedRegex(@"(?<![.\w])[A-Za-z_]\w*")]
    private static partial Regex NamePattern();

    [GeneratedRegex(@"\bpc(?:_resume)?\s*=(?!=)\s*(?<value>[^;]*);")]
    private static partial Regex JumpPattern();

    [GeneratedRegex(@"0x[0-9a-fA-F]+")]
    private static partial Regex AddressPattern();

    // A target computed from a value, not a block address or a PC_ constant.
    [GeneratedRegex(@"(?<!\w)(?!PC_)[A-Za-z_]\w*")]
    private static partial Regex OpaqueTargetPattern();
}
