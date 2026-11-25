// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using FiveStar.Formats.Common;

namespace FiveStar.Formats.Pso;

/// <summary>
/// Collects name hashes from PSO documents, counted per category: <c>struct</c> (struct names above 0xFFFF),
/// <c>member</c>, <c>enum</c>, <c>enumval</c> and <c>value</c> (non-zero hash-typed data found by walking the root).
/// Categories appear in the order they are first met.
/// </summary>
public sealed class PsoHashHarvest
{
    private const uint SynthesizedStructLimit = 0xFFFF;
    private const string HashValuePrefix = "h:";

    private readonly OrderedMap<string, Tally<uint>> _categories = new();

    public OrderedMap<string, Tally<uint>> Categories => _categories;

    public void Add(PsoDocument document)
    {
        foreach (var schema in document.Structs.Values)
        {
            if (schema.Name > SynthesizedStructLimit)
            {
                Category("struct").Add(schema.Name);
            }

            foreach (var member in schema.Members)
            {
                if (member.Name != PsoMember.AnonymousElement)
                {
                    Category("member").Add(member.Name);
                }
            }
        }

        for (var index = 0; index < document.Enums.Count; index++)
        {
            Category("enum").Add(document.Enums.Keys[index]);

            foreach (var value in document.Enums.Values[index])
            {
                Category("enumval").Add(value.Hash);
            }
        }

        AddWalkedValues(document);
    }

    /// <summary>The harvest as a tree: category to (8-digit hex hash to count), most common first.</summary>
    public OrderedMap<string, object?> ToTree()
    {
        var tree = new OrderedMap<string, object?>();

        for (var index = 0; index < _categories.Count; index++)
        {
            var counts = new OrderedMap<string, object?>();

            foreach (var entry in _categories.Values[index].MostCommon())
            {
                counts.Set(entry.Key.ToString("x8", CultureInfo.InvariantCulture), entry.Value);
            }

            tree.Set(_categories.Keys[index], counts);
        }

        return tree;
    }

    private void AddWalkedValues(PsoDocument document)
    {
        var values = new List<uint>();

        try
        {
            CollectHashes(new PsoWalker(document).WalkRoot(PsoWalker.UnlimitedBudget), values);
        }
        catch (PsoException)
        {
            return;
        }

        foreach (var value in values)
        {
            Category("value").Add(value);
        }
    }

    private static void CollectHashes(object? node, List<uint> values)
    {
        switch (node)
        {
            case OrderedMap<string, object?> map:
                foreach (var child in map.Values)
                {
                    CollectHashes(child, values);
                }

                break;
            case List<object?> list:
                foreach (var child in list)
                {
                    CollectHashes(child, values);
                }

                break;
            case string text:
                CollectHash(text, values);
                break;
            default:
                break;
        }
    }

    private static void CollectHash(string text, List<uint> values)
    {
        if (!text.StartsWith(HashValuePrefix, StringComparison.Ordinal))
        {
            return;
        }

        var value = uint.Parse(text.AsSpan(HashValuePrefix.Length), NumberStyles.HexNumber, CultureInfo.InvariantCulture);

        if (value != 0)
        {
            values.Add(value);
        }
    }

    private Tally<uint> Category(string name)
    {
        if (!_categories.TryGetValue(name, out var tally))
        {
            tally = new Tally<uint>();
            _categories.Set(name, tally);
        }

        return tally;
    }
}
