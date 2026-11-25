// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Common;

/// <summary>Counts per key in first-seen order; <c>MostCommon</c> sorts by count and keeps that order for ties.</summary>
public sealed class Tally<TKey>
    where TKey : notnull
{
    private readonly OrderedMap<TKey, long> _counts = new();

    public int Count => _counts.Count;

    public IReadOnlyList<TKey> Keys => _counts.Keys;

    public long this[TKey key]
    {
        get
        {
            _counts.TryGetValue(key, out var count);

            return count;
        }
    }

    public long Total
    {
        get
        {
            long total = 0;

            foreach (var count in _counts.Values)
            {
                total += count;
            }

            return total;
        }
    }

    public void Add(TKey key)
    {
        Add(key, 1);
    }

    public void Add(TKey key, long amount)
    {
        _counts.TryGetValue(key, out var count);
        _counts.Set(key, count + amount);
    }

    /// <summary>Adds every count of <paramref name="other"/>, taking its keys in its order.</summary>
    public void AddAll(Tally<TKey> other)
    {
        for (var index = 0; index < other._counts.Count; index++)
        {
            Add(other._counts.Keys[index], other._counts.Values[index]);
        }
    }

    /// <summary>The entries by descending count; equal counts stay in first-seen order.</summary>
    public IReadOnlyList<KeyValuePair<TKey, long>> MostCommon()
    {
        var entries = new List<KeyValuePair<TKey, long>>(_counts.Count);

        for (var index = 0; index < _counts.Count; index++)
        {
            entries.Add(new KeyValuePair<TKey, long>(_counts.Keys[index], _counts.Values[index]));
        }

        // List.Sort is not stable; the position breaks ties.
        var order = new int[entries.Count];

        for (var index = 0; index < order.Length; index++)
        {
            order[index] = index;
        }

        Array.Sort(order, (left, right) => CompareEntries(entries, left, right));
        var sorted = new List<KeyValuePair<TKey, long>>(entries.Count);

        foreach (var position in order)
        {
            sorted.Add(entries[position]);
        }

        return sorted;
    }

    /// <summary>The first <paramref name="count"/> entries of <c>MostCommon()</c>.</summary>
    public IReadOnlyList<KeyValuePair<TKey, long>> MostCommon(int count)
    {
        var all = MostCommon();

        if (count >= all.Count)
        {
            return all;
        }

        var first = new List<KeyValuePair<TKey, long>>(count);

        for (var index = 0; index < count; index++)
        {
            first.Add(all[index]);
        }

        return first;
    }

    private static int CompareEntries(List<KeyValuePair<TKey, long>> entries, int left, int right)
    {
        var byCount = entries[right].Value.CompareTo(entries[left].Value);

        if (byCount != 0)
        {
            return byCount;
        }

        return left.CompareTo(right);
    }
}
