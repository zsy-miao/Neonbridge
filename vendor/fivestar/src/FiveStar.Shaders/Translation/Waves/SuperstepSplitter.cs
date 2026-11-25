// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Text.RegularExpressions;
using FiveStar.Shaders.Translation.Core;

namespace FiveStar.Shaders.Translation.Waves;

/// <summary>
/// Cuts a mesh program's blocks for the superstep form (rdna_superstep.hlsli): before each wave operation the block
/// deposits the operation's value and bit (<c>vw_ss_put</c>) and stops (<c>pc = PC_WAVE</c>); a new block resumes with the
/// operation replaced by its result. An LDS access becomes a step of its own (the windows of one console wave reach it
/// together and none runs past it before the others did it), and a group barrier inside a block a stop at PC_YIELD.
/// Temporaries a later part of a block reads move to function scope (<see cref="Hoisted"/>).
/// </summary>
internal sealed partial class SuperstepSplitter
{
    // Synthetic block addresses, far above any program's code.
    private const int FirstAddress = 0x40000000;

    private readonly List<string> _hoisted = [];
    private int _next = FirstAddress;

    // The mask the last step deposited, while nothing it reads has been written since; null after any other step.
    private string? _deposited;

    /// <summary>Function-scope declarations of temporaries read across a cut.</summary>
    public IReadOnlyList<string> Hoisted => _hoisted;

    /// <summary>Whether a step deposits several readfirstlane values (rdna_superstep.hlsli's VW_SS_VALUES 4).</summary>
    public bool UsesValues { get; private set; }

    /// <summary>The values one step deposits for readfirstlanes of one mask.</summary>
    public const int FirstLaneValues = 4;

    /// <summary>The cut blocks; throws <see cref="UnsupportedProgramException"/> for a statement it cannot cut.</summary>
    public List<CodeBlock> Split(IReadOnlyList<CodeBlock> blocks)
    {
        var result = new List<CodeBlock>();

        foreach (var block in blocks)
        {
            result.AddRange(SplitBlock(block));
        }

        return result;
    }

    private List<CodeBlock> SplitBlock(CodeBlock block)
    {
        var parts = new List<(int Address, List<string> Lines)> { (block.Address, []) };

        // Other paths reach a block: no step's mask carries into it.
        _deposited = null;

        // A compound statement split over lines is one memory step: a step may not stop inside its braces. One with a
        // wave operation is cut line by line, as before.
        var statements = Statements(block.Lines);

        for (var index = 0; index < statements.Count; index++)
        {
            var statement = statements[index];
            var batch = FirstLanes(statements, index);

            if (batch.Count > 1)
            {
                CutFirstLanes(parts, batch);
                index += batch.Count - 1;

                continue;
            }

            if (WaveCallPattern().IsMatch(statement))
            {
                statement.Split('\n').ToList().ForEach(line => Cut(parts, line));
            }
            else
            {
                Cut(parts, statement);
            }
        }

        Hoist(parts);
        var blocks = new List<CodeBlock>();

        for (var index = 0; index < parts.Count; index++)
        {
            var exit = block.Exit;

            if (index + 1 < parts.Count)
            {
                exit = new BlockExit(BlockExitKind.Yield, parts[index + 1].Address, null, null);
            }

            blocks.Add(new CodeBlock(parts[index].Address, parts[index].Lines, exit));
        }

        return blocks;
    }

    /// <summary>
    /// The readfirstlanes of one mask from <paramref name="index"/> on, up to <see cref="FirstLaneValues"/>, while none
    /// writes what a later one reads (a waterfall loop reads a descriptor's dwords this way).
    /// </summary>
    private static List<Match> FirstLanes(List<string> statements, int index)
    {
        var batch = new List<Match>();

        for (var next = index; next < statements.Count && batch.Count < FirstLaneValues; next++)
        {
            var match = FirstLanePattern().Match(statements[next]);

            if (!match.Success || (batch.Count > 0 && match.Groups["mask"].Value != batch[0].Groups["mask"].Value))
            {
                break;
            }

            var reads = new[] { match.Groups["value"].Value, match.Groups["mask"].Value };

            if (batch.Any(earlier => reads.Contains(earlier.Groups["target"].Value)))
            {
                break;
            }

            batch.Add(match);
        }

        return batch;
    }

    /// <summary>One step for several readfirstlanes of one mask: each thread deposits their values together.</summary>
    private void CutFirstLanes(List<(int Address, List<string> Lines)> parts, List<Match> batch)
    {
        var values = batch.Select(match => match.Groups["value"].Value).ToList();

        while (values.Count < FirstLaneValues)
        {
            values.Add("0u");
        }

        var mask = batch[0].Groups["mask"].Value;
        parts[^1].Lines.Add($"  vw_ss_put_values(uint4({string.Join(", ", values)}), {mask});");
        Stop(parts, "PC_WAVE");
        _deposited = mask;
        UsesValues = true;

        for (var index = 0; index < batch.Count; index++)
        {
            var line = $"  {batch[index].Groups["target"].Value} = vw_ss_first_value({index}u);";
            parts[^1].Lines.Add(line);
            Forget(line);
        }
    }

    /// <summary>The block's lines joined into statements whose braces balance.</summary>
    private static List<string> Statements(IReadOnlyList<string> lines)
    {
        var statements = new List<string>();
        var open = new List<string>();
        var depth = 0;

        foreach (var line in lines)
        {
            open.Add(line);
            depth += line.Count(character => character == '{') - line.Count(character => character == '}');

            if (depth <= 0)
            {
                statements.Add(string.Join("\n", open));
                open.Clear();
                depth = 0;
            }
        }

        if (open.Count > 0)
        {
            statements.Add(string.Join("\n", open));
        }

        return statements;
    }

    /// <summary>Appends a statement to the last part, first stopping where it needs a step.</summary>
    private void Cut(List<(int Address, List<string> Lines)> parts, string line)
    {
        var calls = WaveCallPattern().Matches(line);
        var shared = SharedPattern().IsMatch(line);

        if (calls.Count > 1 || (calls.Count > 0 && line.Contains('\n', StringComparison.Ordinal)) || ((calls.Count > 0 || shared) && LoopPattern().IsMatch(line)))
        {
            throw new UnsupportedProgramException($"superstep: cannot stop inside {line.Trim()}");
        }

        if (line.Trim() == "GroupMemoryBarrierWithGroupSync();")
        {
            Stop(parts, "PC_YIELD");

            return;
        }

        if (calls.Count == 1)
        {
            var (deposit, replaced, bit) = Operation(line, calls[0]);

            if (bit is not null && bit == _deposited)
            {
                // The last step's mask is this one's: a ballot's .y after its .x, an any of the mask a ballot took.
                parts[^1].Lines.Add(replaced);
                Forget(line);

                return;
            }

            parts[^1].Lines.Add($"  {deposit};");
            Stop(parts, "PC_WAVE");
            _deposited = bit;
            parts[^1].Lines.Add(replaced);
            Forget(line);

            return;
        }

        if (shared)
        {
            Stop(parts, "PC_WAVE");
            parts[^1].Lines.Add(line);
            Stop(parts, "PC_WAVE");

            return;
        }

        parts[^1].Lines.Add(line);
        Forget(line);
    }

    /// <summary>Forgets the last step's mask when <paramref name="line"/> writes a name it reads.</summary>
    private void Forget(string line)
    {
        if (_deposited is null)
        {
            return;
        }

        foreach (Match write in AssignmentPattern().Matches(line))
        {
            if (Regex.IsMatch(_deposited, $@"(?<![.\w]){Regex.Escape(write.Groups["name"].Value)}\b"))
            {
                _deposited = null;

                return;
            }
        }
    }

    private void Stop(List<(int Address, List<string> Lines)> parts, string stop)
    {
        var address = _next;
        _next++;
        parts[^1].Lines.Add($"  pc_resume = {Hlsl.Hex(address)}; pc = {stop};");
        parts.Add((address, []));
        _deposited = null;
    }

    /// <summary>
    /// The deposit of a wave operation, the statement with the operation replaced by its result, and the mask whose
    /// ballot the step leaves readable (null for a lane read, and for readfirstlane, whose value a later one may not share).
    /// </summary>
    private static (string Deposit, string Replaced, string? Bit) Operation(string line, Match call)
    {
        var open = call.Index + call.Length;
        var close = Closing(line, open - 1);
        var arguments = TopLevelArguments(line[open..close]);
        var name = call.Groups[1].Value;
        var (deposit, result) = name switch
        {
            "vw_read" => ($"vw_ss_put({arguments[0]}, false, {arguments[1]})", "vw_ss_read()"),
            "readfirstlane" => ($"vw_ss_put({arguments[0]}, {arguments[1]}, 0u)", "vw_ss_first()"),
            "vw_any" => ($"vw_ss_put(0u, {arguments[0]}, 0u)", "vw_ss_any()"),
            "vw_count" => ($"vw_ss_put(0u, {arguments[0]}, 0u)", "vw_ss_count()"),
            _ => ($"vw_ss_put(0u, {arguments[0]}, 0u)", "vw_ss_ballot()"),
        };
        string? bit = null;

        if (name is "vw_any" or "vw_count" or "vw_ballot")
        {
            bit = Unparenthesized(arguments[0]);
        }

        return (deposit, line[..call.Index] + result + line[(close + 1)..], bit);
    }

    private static string Unparenthesized(string text)
    {
        var trimmed = text.Trim();

        while (trimmed.Length > 1 && trimmed[0] == '(' && Closing(trimmed, 0) == trimmed.Length - 1)
        {
            trimmed = trimmed[1..^1].Trim();
        }

        return trimmed;
    }

    /// <summary>Moves temporaries declared before a cut and read after it to function scope.</summary>
    private void Hoist(List<(int Address, List<string> Lines)> parts)
    {
        for (var part = 0; part + 1 < parts.Count; part++)
        {
            var later = string.Join("\n", parts.Skip(part + 1).SelectMany(next => next.Lines));
            var lines = parts[part].Lines;

            for (var index = 0; index < lines.Count; index++)
            {
                var declaration = DeclarationPattern().Match(lines[index]);

                if (!declaration.Success || !Regex.IsMatch(later, $@"\b{declaration.Groups["name"].Value}\b"))
                {
                    continue;
                }

                if (!ValueTypePattern().IsMatch(declaration.Groups["type"].Value))
                {
                    throw new UnsupportedProgramException($"superstep: {declaration.Groups["name"].Value} is read across a step");
                }

                _hoisted.Add($"  {declaration.Groups["type"].Value} {declaration.Groups["name"].Value};");
                lines[index] = "  " + lines[index].Trim()[(declaration.Groups["type"].Value.Length + 1)..];
            }
        }
    }

    private static List<string> TopLevelArguments(string text)
    {
        var arguments = new List<string>();
        var depth = 0;
        var start = 0;

        for (var index = 0; index < text.Length; index++)
        {
            depth += Depth(text[index]);

            if (depth == 0 && text[index] == ',')
            {
                arguments.Add(text[start..index].Trim());
                start = index + 1;
            }
        }

        arguments.Add(text[start..].Trim());

        return arguments;
    }

    private static int Closing(string text, int open)
    {
        var depth = 0;

        for (var index = open; index < text.Length; index++)
        {
            depth += Depth(text[index]);

            if (depth == 0)
            {
                return index;
            }
        }

        throw new UnsupportedProgramException($"superstep: unbalanced {text.Trim()}");
    }

    private static int Depth(char character)
    {
        if (character == '(')
        {
            return 1;
        }

        if (character == ')')
        {
            return -1;
        }

        return 0;
    }

    [GeneratedRegex(@"\b(vw_read|vw_ballot|vw_any|vw_count|readfirstlane)\(")]
    private static partial Regex WaveCallPattern();

    [GeneratedRegex(@"^\s*(?<target>[A-Za-z_]\w*) = readfirstlane\((?<value>[A-Za-z_]\w*), (?<mask>[A-Za-z_]\w*)\);\s*$")]
    private static partial Regex FirstLanePattern();

    [GeneratedRegex(@"(?<![.\w])(?<name>[A-Za-z_]\w*)\s*(?:\[[^\]]*\])?\s*(?:[-+*/&|^]|<<|>>)?=(?!=)")]
    private static partial Regex AssignmentPattern();

    [GeneratedRegex(@"\bg_lds\[|\b\w*(?:Store|Atomic|Interlocked)\w*\s*\(")]
    private static partial Regex SharedPattern();

    [GeneratedRegex(@"\b(?:for|while)\s*\(")]
    private static partial Regex LoopPattern();

    [GeneratedRegex(@"^\s*(?<type>[A-Za-z_]\w*(?:<[^>]*>)?)\s+(?<name>_\w+)\s*=")]
    private static partial Regex DeclarationPattern();

    [GeneratedRegex(@"^(?:u?int[234]?|float[234]?|bool|u?int64_t)$")]
    private static partial Regex ValueTypePattern();
}
