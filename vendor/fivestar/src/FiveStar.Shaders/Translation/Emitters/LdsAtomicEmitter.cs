// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Translation.Core;
using static FiveStar.Shaders.Translation.Emitters.Operands;

namespace FiveStar.Shaders.Translation.Emitters;

/// <summary>
/// LDS atomics (ds_add, ds_min, ..., ds_wrxchg, ds_cmpst), with or without _rtn (returning the old value). g_lds is
/// uint, so the signed min/max run a compare-exchange loop with a signed compare.
/// </summary>
internal sealed class LdsAtomicEmitter
{
    // Without _rtn the operands are vaddr, data[, data1]; with _rtn vdst comes first.
    private const int ReturningOffset = 1;
    private const int CompareOperand = 1;
    private const int SwapOperand = 2;

    private readonly TranslationState _state;
    private readonly OperandAccess _operands;

    public LdsAtomicEmitter(TranslationState state, OperandAccess operands)
    {
        _state = state;
        _operands = operands;
    }

    /// <summary>Translates the access when it is an LDS atomic; false otherwise.</summary>
    public bool TryTranslate(LdsAccess access)
    {
        if (access.Kind is "cmpst_b32" or "cmpst_rtn_b32")
        {
            CompareStore(access);

            return true;
        }

        var operation = access.Kind;

        if (operation != "wrxchg_rtn_b32")
        {
            operation = operation.Replace("_rtn", string.Empty, StringComparison.Ordinal);
        }

        var function = InterlockedFunction(operation);

        if (function is null)
        {
            return false;
        }

        Atomic(access, operation, function);

        return true;
    }

    private static string? InterlockedFunction(string operation)
    {
        switch (operation)
        {
            case "add_u32":
                return "InterlockedAdd";
            case "min_u32":
            case "min_i32":
                return "InterlockedMin";
            case "max_u32":
            case "max_i32":
                return "InterlockedMax";
            case "and_b32":
                return "InterlockedAnd";
            case "or_b32":
                return "InterlockedOr";
            case "xor_b32":
                return "InterlockedXor";
            case "wrxchg_rtn_b32":
                return "InterlockedExchange";
            default:
                return null;
        }
    }

    /// <summary>ds_cmpst: if (mem == data0) mem = data1; returns the old value.</summary>
    private void CompareStore(LdsAccess access)
    {
        var returns = access.Kind.Contains("_rtn", StringComparison.Ordinal);
        var first = returns ? ReturningOffset : 0;
        var a = access.Operands;
        var address = _operands.SnapOne($"({_operands.Read(a[first])} + {access.Offset}u)");
        var name = $"_la{_state.NextTemporary()}";
        var compare = _operands.Read(a[first + CompareOperand]);

        _state.Emit($"  uint {name} = 0; if (active) InterlockedCompareExchange(g_lds[{address} >> 2], {compare}, {_operands.Read(a[first + SwapOperand])}, {name});");

        if (returns)
        {
            _operands.Write(a[Destination], name);
        }
    }

    private void Atomic(LdsAccess access, string operation, string function)
    {
        var returns = access.Kind.Contains("_rtn", StringComparison.Ordinal);
        var first = returns ? ReturningOffset : 0;
        var a = access.Operands;
        var address = _operands.SnapOne($"({_operands.Read(a[first])} + {access.Offset}u)");
        var name = $"_la{_state.NextTemporary()}";
        var data = a[first + 1];

        if (operation is "min_i32" or "max_i32")
        {
            var signed = operation[.."min".Length];
            _state.Emit($"  uint {name} = 0; if (active) {{ uint _v = {_operands.Read(data)}; {name} = g_lds[{address} >> 2]; [loop] for (;;) {{ uint _p; "
                + $"InterlockedCompareExchange(g_lds[{address} >> 2], {name}, asuint({signed}(asint({name}), asint(_v))), _p); "
                + $"if (_p == {name}) break; {name} = _p; }} }}");
        }
        else
        {
            _state.Emit($"  uint {name} = 0; if (active) {function}(g_lds[{address} >> 2], {_operands.Read(data)}, {name});");
        }

        if (returns)
        {
            _operands.Write(a[Destination], name);
        }
    }
}
