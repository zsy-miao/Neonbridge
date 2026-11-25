// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Debugger.Session;

/// <summary>An image in the debuggee: loaded by the OS, or mapped by fivestar-loader itself (then offsets are RVAs).</summary>
internal sealed record LoadedModule(string Name, string Path, ulong Base, ulong Size, bool MappedByLoader)
{
    public bool Contains(ulong address)
    {
        return address >= Base && address - Base < Size;
    }
}
