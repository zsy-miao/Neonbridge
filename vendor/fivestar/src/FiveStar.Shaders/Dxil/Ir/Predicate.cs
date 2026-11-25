// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Dxil.Ir;

/// <summary>LLVM compare predicates: floats 0-15, integers 32-41.</summary>
public enum Predicate
{
    FloatFalse = 0,
    FloatOrderedEqual = 1,
    FloatOrderedGreater = 2,
    FloatOrderedGreaterOrEqual = 3,
    FloatOrderedLess = 4,
    FloatOrderedLessOrEqual = 5,
    FloatOrderedNotEqual = 6,
    FloatOrdered = 7,
    FloatUnordered = 8,
    FloatUnorderedEqual = 9,
    FloatUnorderedGreater = 10,
    FloatUnorderedGreaterOrEqual = 11,
    FloatUnorderedLess = 12,
    FloatUnorderedLessOrEqual = 13,
    FloatUnorderedNotEqual = 14,
    FloatTrue = 15,
    Equal = 32,
    NotEqual = 33,
    UnsignedGreater = 34,
    UnsignedGreaterOrEqual = 35,
    UnsignedLess = 36,
    UnsignedLessOrEqual = 37,
    SignedGreater = 38,
    SignedGreaterOrEqual = 39,
    SignedLess = 40,
    SignedLessOrEqual = 41,
}
