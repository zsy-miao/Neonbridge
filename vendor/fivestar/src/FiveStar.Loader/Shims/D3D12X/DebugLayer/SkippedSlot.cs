// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;

namespace FiveStar.Loader.Shims.D3D12X.DebugLayer;

/// <summary>
/// A vtable slot <c>--d3d12-skip</c> turns into a call that does nothing and returns 0 (S_OK), to bisect GPU work.
/// Written <c>interface:slot</c>, the interface by its <see cref="XboxInterfaceKind"/> name or an alias
/// (<c>list</c>, <c>queue</c>, <c>allocator</c>).
/// </summary>
internal sealed record SkippedSlot(XboxInterfaceKind Kind, int Slot)
{
    /// <summary>Parses a comma-separated list such as <c>list:51,list:52,queue:35</c>.</summary>
    public static IReadOnlyList<SkippedSlot> ParseList(string text)
    {
        var slots = new List<SkippedSlot>();

        foreach (string item in text.Split(',', StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries))
        {
            slots.Add(Parse(item));
        }

        return slots;
    }

    public static SkippedSlot Parse(string item)
    {
        int colon = item.IndexOf(':', StringComparison.Ordinal);

        if (colon <= 0 || !int.TryParse(item.AsSpan(colon + 1), NumberStyles.Integer, CultureInfo.InvariantCulture, out int slot) || slot < 0)
        {
            throw new FormatException($"'{item}' is not interface:slot");
        }

        string name = item[..colon];
        var kind = name.ToLowerInvariant() switch
        {
            "list" => XboxInterfaceKind.CommandList,
            "queue" => XboxInterfaceKind.CommandQueue,
            "allocator" => XboxInterfaceKind.CommandAllocator,
            _ => Enum.Parse<XboxInterfaceKind>(name, true),
        };

        return new SkippedSlot(kind, slot);
    }

    public override string ToString()
    {
        return $"{Kind}:{Slot}";
    }
}
