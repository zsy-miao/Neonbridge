// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Dxil.Lowering;

/// <summary>An entry parameter or return value: its type and its signature elements (by field for a struct).</summary>
public sealed class EntryBinding
{
    public EntryBinding(HlslType type)
    {
        Type = type;
    }

    public HlslType Type { get; }

    public List<ElementSlot> Elements { get; } = [];

    public Dictionary<string, ElementSlot> Fields { get; } = new(StringComparer.Ordinal);

    /// <summary>The element holding flattened component <paramref name="component"/>.</summary>
    public ElementSlot Holding(int component)
    {
        return Elements.Last(slot => slot.First <= component);
    }
}
