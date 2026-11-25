// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Diagnostics;

/// <summary>
/// A hint of whose an anonymous allocation is: the OS-loaded modules that the pointers in its first page point into
/// (vtables, callbacks and globals an allocator keeps in its block headers), most frequent first.
/// </summary>
internal sealed unsafe class ModulePointers
{
    private const int ProbeBytes = 4096;
    private const int MaxModules = 2;

    private readonly (ulong Start, ulong End, string Name)[] _modules;

    private ModulePointers((ulong Start, ulong End, string Name)[] modules)
    {
        _modules = modules;
    }

    /// <summary>The modules loaded now, sorted by address.</summary>
    public static ModulePointers Loaded()
    {
        var modules = new List<(ulong Start, ulong End, string Name)>();

        foreach (ProcessModule module in Process.GetCurrentProcess().Modules)
        {
            ulong start = (ulong)module.BaseAddress;
            modules.Add((start, start + (ulong)module.ModuleMemorySize, module.ModuleName));
        }

        return new ModulePointers([.. modules.OrderBy(m => m.Start)]);
    }

    /// <summary>"module xN" for the modules the first page of <paramref name="allocation"/> points into; empty for none.</summary>
    public string Hint(nint allocation)
    {
        var page = stackalloc ulong[ProbeBytes / sizeof(ulong)];
        nuint read = 0;

        // ReadProcessMemory: the allocation may be freed since the walk.
        if (Kernel32.ReadProcessMemory(Kernel32.GetCurrentProcess(), allocation, page, ProbeBytes, &read) == 0)
        {
            return string.Empty;
        }

        var counts = new Dictionary<string, int>(StringComparer.Ordinal);

        for (int i = 0; i < (int)read / sizeof(ulong); i++)
        {
            string? name = Find(page[i]);

            if (name is not null)
            {
                counts[name] = counts.GetValueOrDefault(name) + 1;
            }
        }

        return string.Join(" ", counts.OrderByDescending(c => c.Value).Take(MaxModules).Select(c => $"{c.Key} x{c.Value}"));
    }

    private string? Find(ulong value)
    {
        int low = 0;
        int high = _modules.Length - 1;

        while (low <= high)
        {
            int middle = low + ((high - low) >> 1);
            var module = _modules[middle];

            if (value < module.Start)
            {
                high = middle - 1;
            }
            else if (value >= module.End)
            {
                low = middle + 1;
            }
            else
            {
                return module.Name;
            }
        }

        return null;
    }
}
