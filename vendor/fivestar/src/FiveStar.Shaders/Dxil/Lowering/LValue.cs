// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Dxil.Ir;

namespace FiveStar.Shaders.Dxil.Lowering;

/// <summary>
/// A place that can be read and written, component by component:
/// <list type="bullet">
/// <item>slots: <see cref="Slots"/> holds each component's SSA slot;</item>
/// <item>memory: <see cref="Indices"/> holds each component's element index in the array <see cref="Variable"/> points to;</item>
/// <item>a dynamically indexed element of slots: <see cref="Choices"/> are the slots of every candidate element and
/// <see cref="Selector"/> picks one;</item>
/// <item>cbuffer: <see cref="Indices"/> holds each component's dword index in the buffer;</item>
/// <item>mesh output: <see cref="Indices"/> holds each component's index in the array (in one element when
/// <see cref="ArrayIndex"/> is the element's index).</item>
/// </list>
/// </summary>
public sealed class LValue
{
    public LValue(HlslType type, Variable variable)
    {
        Type = type;
        Variable = variable;
    }

    public HlslType Type { get; }

    public Variable Variable { get; }

    public IReadOnlyList<int> Slots { get; init; } = [];

    public IReadOnlyList<Value> Indices { get; init; } = [];

    public IReadOnlyList<IReadOnlyList<int>> Choices { get; init; } = [];

    public Value? Selector { get; init; }

    /// <summary>The element of a mesh output array, when it is not a constant.</summary>
    public Value? ArrayIndex { get; init; }

    public bool IsDynamic => Selector is not null;
}
