// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Debugger.Session;

/// <summary>The images of one debuggee by base address.</summary>
internal sealed class ModuleList
{
    private readonly SortedList<ulong, LoadedModule> _modules = [];

    public IEnumerable<LoadedModule> All => _modules.Values;

    public void Add(LoadedModule module)
    {
        _modules[module.Base] = module;
    }

    public LoadedModule? Remove(ulong baseAddress)
    {
        if (!_modules.Remove(baseAddress, out var module))
        {
            return null;
        }

        return module;
    }

    public LoadedModule? Find(ulong address)
    {
        foreach (var module in _modules.Values)
        {
            if (module.Contains(address))
            {
                return module;
            }
        }

        return null;
    }
}
