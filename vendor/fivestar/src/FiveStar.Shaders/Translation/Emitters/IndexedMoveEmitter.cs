// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Translation.Core;
using static FiveStar.Shaders.Translation.Emitters.Operands;

namespace FiveStar.Shaders.Translation.Emitters;

/// <summary>
/// VGPR-indexed moves (index in M0): v_movrels vD = v[S + m0]; v_movreld v[D + m0] = vS. HLSL has no indexed
/// registers, so the index selects among every VGPR from the base up with a switch.
/// </summary>
internal sealed class IndexedMoveEmitter
{
    // Programs without a VGPR count in the header may use all 256 VGPRs.
    private const int MaxVgprs = 256;

    private readonly TranslationState _state;
    private readonly OperandAccess _operands;

    public IndexedMoveEmitter(TranslationState state, OperandAccess operands)
    {
        _state = state;
        _operands = operands;
    }

    /// <summary>Translates the instruction when it is an indexed move; false otherwise.</summary>
    public bool TryTranslate(string op, IReadOnlyList<Operand> a)
    {
        switch (op)
        {
            case "v_movrels_b32":
                MoveFromIndexed(a);
                break;
            case "v_movreld_b32":
                MoveToIndexed(a);
                break;
            case "v_movrelsd_b32":
                throw new UnsupportedProgramException(op);
            default:
                return false;
        }

        return true;
    }

    private int VgprCount()
    {
        var count = _state.Config.VgprCount ?? 0;
        var result = count == 0 ? MaxVgprs : count;

        return result;
    }

    private string Index()
    {
        return _operands.SnapOne(_operands.Read(OperandParser.Parse("m0")));
    }

    private void MoveFromIndexed(IReadOnlyList<Operand> a)
    {
        var index = Index();
        var source = a[Source0];
        var cases = new List<string>();

        for (var offset = 0; offset < Math.Max(1, VgprCount() - source.Low); offset++)
        {
            var register = RegisterKey.Vgpr(source.Low + offset);
            _state.Touch(register);
            cases.Add($"case {offset}: _mr = {register.Name}; break;");
        }

        var name = $"_mr{_state.NextTemporary()}";
        _state.Emit($"  uint {name} = 0; {{ uint _mr = 0; switch ({index}) {{ {string.Join(' ', cases)} }} {name} = _mr; }}");
        _operands.Write(a[Destination], name);
    }

    private void MoveToIndexed(IReadOnlyList<Operand> a)
    {
        var index = Index();
        var destination = a[Destination];
        var value = _operands.SnapOne(_operands.Read(a[Source0]));
        var cases = new List<string>();

        for (var offset = 0; offset < Math.Max(1, VgprCount() - destination.Low); offset++)
        {
            var register = RegisterKey.Vgpr(destination.Low + offset);
            _state.Touch(register);
            cases.Add($"case {offset}: if (active) {register.Name} = {value}; break;");
        }

        _state.Emit($"  switch ({index}) {{ {string.Join(' ', cases)} }}");
    }
}
