// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Translation.Core;

namespace FiveStar.Shaders.Translation.Emitters;

/// <summary>
/// The VALU instructions beyond one-expression operations: carries and 64-bit integers, f64, cross-lane moves,
/// indexed VGPR moves and 16-bit / packed / mixed-precision math.
/// </summary>
internal sealed class ValuExtendedEmitter
{
    private readonly WideIntegerEmitter _integers;
    private readonly CrossLaneEmitter _crossLane;
    private readonly IndexedMoveEmitter _indexed;
    private readonly HalfPrecisionEmitter _halves;

    public ValuExtendedEmitter(
        WideIntegerEmitter integers, CrossLaneEmitter crossLane, IndexedMoveEmitter indexed, HalfPrecisionEmitter halves)
    {
        _integers = integers;
        _crossLane = crossLane;
        _indexed = indexed;
        _halves = halves;
    }

    /// <summary>Translates the instruction when it belongs to one of these families; false otherwise.</summary>
    public bool TryTranslate(string op, IReadOnlyList<Operand> a, InstructionModifiers modifiers)
    {
        if (_integers.TryTranslate(op, a))
        {
            return true;
        }

        if (_indexed.TryTranslate(op, a))
        {
            return true;
        }

        if (_crossLane.TryTranslate(op, a))
        {
            return true;
        }

        return _halves.TryTranslate(op, a, modifiers);
    }
}
