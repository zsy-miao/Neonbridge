// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Translation.Core;
using static FiveStar.Shaders.Translation.Emitters.Operands;

namespace FiveStar.Shaders.Translation.Emitters;

/// <summary>
/// DPP (data-parallel primitives): src0 is read from another lane of the same row (quad_perm, row shifts, rotates,
/// mirrors) or of the same 8-lane group (dpp8). Lanes outside row_mask/bank_mask, and row shifts that run off the row
/// without bound_ctrl, keep their old destination value.
/// </summary>
internal sealed class DppRewriter
{
    private const string LaneIndex = WaveOps.Lane;
    private const string InvalidLane = "0xffffffffu";

    // Lane groups the patterns stay inside: quads, half rows, 16-lane rows.
    private const int QuadLanes = 4;
    private const int HalfRowLanes = 8;
    private const int RowLanes = 16;

    // dpp8: one 3-bit lane select per lane of an 8-lane group.
    private const int Dpp8Lanes = 8;
    private const int Dpp8MiddleLanes = 6;
    private const int AllRowsOrBanks = 0xf;

    private readonly TranslationState _state;
    private readonly OperandAccess _operands;

    public DppRewriter(TranslationState state, OperandAccess operands)
    {
        _state = state;
        _operands = operands;
    }

    /// <summary>The operand list with src0 replaced by the value read from the selected lane.</summary>
    public List<Operand> Apply(IReadOnlyList<Operand> a, InstructionModifiers modifiers)
    {
        var x = _operands.Read(a[Source0]);
        var keep = LaneMasks(modifiers);

        if (modifiers.Has("dpp8"))
        {
            // dpp8 has no row or bank masks.
            var lanes = ArgumentList.Bits(modifiers.Value("dpp8"));
            var lane = _operands.SnapOne($"(({LaneIndex} & ~7u) | {Dpp8Select(lanes)})");
            var read = _operands.SnapOne($"{WaveOps.ReadLane}({x}, {lane})");
            _state.Reach(Dpp8Lanes);

            return Replace(a, read);
        }

        string value;

        if (modifiers.Has("quad_perm"))
        {
            var lanes = ArgumentList.Bits(modifiers.Value("quad_perm"));
            value = $"quad_perm({x}, {string.Join(", ", lanes)})";
            _state.Reach(QuadLanes);
        }
        else
        {
            value = RowShift(x, modifiers, keep);
        }

        var source = _operands.SnapOne(value);

        if (keep.Count > 0)
        {
            var name = $"_dk{_state.NextTemporary()}";
            _state.Emit($"  bool {name} = {string.Join(" && ", keep)};");
            _state.DppKeep = name;
        }

        return Replace(a, source);
    }

    private static List<string> LaneMasks(InstructionModifiers modifiers)
    {
        const int RowShift = 4;
        const int BankShift = 2;

        var rowMask = modifiers.Hex("row_mask", AllRowsOrBanks);
        var bankMask = modifiers.Hex("bank_mask", AllRowsOrBanks);
        var keep = new List<string>();

        if (rowMask != AllRowsOrBanks)
        {
            keep.Add($"((({Hlsl.HexUint(rowMask)} >> (({LaneIndex} >> {RowShift}) & 3u)) & 1u) != 0)");
        }

        if (bankMask != AllRowsOrBanks)
        {
            keep.Add($"((({Hlsl.HexUint(bankMask)} >> (({LaneIndex} >> {BankShift}) & 3u)) & 1u) != 0)");
        }

        return keep;
    }

    private static string Dpp8Select(List<int> lanes)
    {
        var select = $"(({LaneIndex} & 7u) == 0 ? {lanes[0]}u : ";
        var middle = Enumerable.Range(1, Dpp8MiddleLanes).Select(lane => $"({LaneIndex} & 7u) == {lane} ? {lanes[lane]}u");

        return select + string.Join(" : ", middle) + $" : {lanes[Dpp8Lanes - 1]}u)";
    }

    /// <summary>Row shifts, rotates and mirrors: reads lane (row base | source); an invalid source lane reads 0.</summary>
    private string RowShift(string x, InstructionModifiers modifiers, List<string> keep)
    {
        var control = modifiers.Keys.FirstOrDefault(key => key.StartsWith("row_", StringComparison.Ordinal) && key != "row_mask")
            ?? throw new UnsupportedProgramException("dpp control");
        var amountText = modifiers.Value(control);
        var amount = 0;

        if (amountText is not null && amountText.All(char.IsAsciiDigit))
        {
            amount = int.Parse(amountText, System.Globalization.CultureInfo.InvariantCulture);
        }

        var source = _operands.SnapOne(RowSource(control, amount));
        var read = _operands.SnapOne($"{WaveOps.ReadLane}({x}, {source} == {InvalidLane} ? 0u : {source})");
        var reach = RowLanes;

        if (control == "row_half_mirror")
        {
            reach = HalfRowLanes;
        }

        _state.Reach(reach);

        if (!modifiers.Has("bound_ctrl") && control is "row_shr" or "row_shl")
        {
            // Without bound_ctrl, a lane whose source is outside the row does not write.
            keep.Add($"({source} != {InvalidLane})");
        }

        return $"({source} == {InvalidLane} ? 0u : {read})";
    }

    private static string RowSource(string control, int amount)
    {
        var row = $"({LaneIndex} & ~15u)";
        var lane = $"({LaneIndex} & 15u)";

        switch (control)
        {
            case "row_shr":
                return $"({lane} >= {amount}u ? {LaneIndex} - {amount}u : {InvalidLane})";
            case "row_shl":
                return $"({lane} + {amount}u < 16u ? {LaneIndex} + {amount}u : {InvalidLane})";
            case "row_ror":
                return $"({row} | (({lane} + 16u - {amount}u) & 15u))";
            case "row_mirror":
                return $"({row} | (15u - {lane}))";
            case "row_half_mirror":
                return $"(({LaneIndex} & ~7u) | (7u - ({LaneIndex} & 7u)))";
            default:
                throw new UnsupportedProgramException($"dpp {control}");
        }
    }

    private static List<Operand> Replace(IReadOnlyList<Operand> a, string source)
    {
        var operands = a.ToList();
        operands[Source0] = Operand.FromExpression(source).WithModifiersOf(a[Source0]);

        return operands;
    }
}
