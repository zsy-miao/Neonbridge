// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Dxil.Lowering;

/// <summary>
/// Name lookup: the globals, then nested blocks. An inlined function sees the globals and its own blocks only, so
/// <see cref="Isolate"/> starts a fresh chain and <see cref="Restore"/> returns to the caller's.
/// </summary>
public sealed class Scope
{
    private readonly Dictionary<string, Variable> _globals = new(StringComparer.Ordinal);
    // A block's names are made on its first declaration; most blocks declare none.
    private List<Dictionary<string, Variable>?> _blocks = [];

    public void DeclareGlobal(Variable variable)
    {
        _globals[variable.Name] = variable;
    }

    public void Declare(Variable variable)
    {
        if (_blocks.Count == 0)
        {
            DeclareGlobal(variable);

            return;
        }

        _blocks[^1] ??= new Dictionary<string, Variable>(StringComparer.Ordinal);
        _blocks[^1]![variable.Name] = variable;
    }

    public void Push()
    {
        _blocks.Add(null);
    }

    public void Pop()
    {
        _blocks.RemoveAt(_blocks.Count - 1);
    }

    public Variable? Find(string name)
    {
        for (var index = _blocks.Count - 1; index >= 0; index--)
        {
            var names = _blocks[index];

            if (names is not null && names.TryGetValue(name, out var variable))
            {
                return variable;
            }
        }

        return _globals.GetValueOrDefault(name);
    }

    /// <summary>Starts an empty chain above the globals; returns the caller's chain for <see cref="Restore"/>.</summary>
    public List<Dictionary<string, Variable>?> Isolate()
    {
        var saved = _blocks;
        _blocks = [];

        return saved;
    }

    public void Restore(List<Dictionary<string, Variable>?> saved)
    {
        _blocks = saved;
    }
}
