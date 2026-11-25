// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Common;

/// <summary>
/// A map that keeps keys in first-insertion order; setting an existing key replaces its value in place. Format
/// tables with duplicate names (two schema entries with one hash) resolve to the last entry at the first position.
/// Small maps search linearly; a hash index is built once they grow.
/// </summary>
public sealed class OrderedMap<TKey, TValue>
    where TKey : notnull
{
    private const int LinearLimit = 8;

    private readonly List<TKey> _keys = [];
    private readonly List<TValue> _values = [];
    private readonly EqualityComparer<TKey> _comparer = EqualityComparer<TKey>.Default;
    private Dictionary<TKey, int>? _index;

    public int Count => _keys.Count;

    public IReadOnlyList<TKey> Keys => _keys;

    public IReadOnlyList<TValue> Values => _values;

    public TValue this[TKey key]
    {
        get
        {
            var position = Find(key);

            if (position < 0)
            {
                throw new KeyNotFoundException();
            }

            return _values[position];
        }

        set => Set(key, value);
    }

    public void Set(TKey key, TValue value)
    {
        var position = Find(key);

        if (position >= 0)
        {
            _values[position] = value;

            return;
        }

        _index?.Add(key, _keys.Count);
        _keys.Add(key);
        _values.Add(value);

        if (_index is null && _keys.Count > LinearLimit)
        {
            BuildIndex();
        }
    }

    public bool ContainsKey(TKey key)
    {
        return Find(key) >= 0;
    }

    public bool TryGetValue(TKey key, out TValue value)
    {
        var position = Find(key);

        if (position >= 0)
        {
            value = _values[position];

            return true;
        }

        value = default!;

        return false;
    }

    private int Find(TKey key)
    {
        if (_index is not null)
        {
            return _index.GetValueOrDefault(key, -1);
        }

        for (var position = 0; position < _keys.Count; position++)
        {
            if (_comparer.Equals(_keys[position], key))
            {
                return position;
            }
        }

        return -1;
    }

    private void BuildIndex()
    {
        _index = new Dictionary<TKey, int>(_keys.Count);

        for (var position = 0; position < _keys.Count; position++)
        {
            _index[_keys[position]] = position;
        }
    }
}
