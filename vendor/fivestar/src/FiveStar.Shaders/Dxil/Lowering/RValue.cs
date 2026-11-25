// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Dxil.Ir;

namespace FiveStar.Shaders.Dxil.Lowering;

/// <summary>
/// A computed value: its HLSL type and its flattened scalar components (a resource has one, its handle). A literal's
/// component is an i64 or double constant that takes a type when it meets one.
/// </summary>
public sealed record RValue(HlslType Type, IReadOnlyList<Value> Components)
{
    public static RValue Void { get; } = new(HlslType.Void, []);

    public Value Scalar => Components[0];

    public static RValue Of(HlslType type, Value component)
    {
        return new RValue(type, [component]);
    }
}
