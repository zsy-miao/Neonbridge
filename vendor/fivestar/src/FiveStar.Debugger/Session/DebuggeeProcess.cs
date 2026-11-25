// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Debugger.Symbols;

namespace FiveStar.Debugger.Session;

/// <summary>One debugged process: its handle (owned by the system), threads, images and symbol handler.</summary>
internal sealed class DebuggeeProcess : IDisposable
{
    private readonly ModuleList _modules = new();
    private readonly Dictionary<uint, nint> _threads = [];
    private readonly SymbolSession _symbols;

    public DebuggeeProcess(uint id, nint handle, SymbolSession symbols)
    {
        Id = id;
        Handle = handle;
        _symbols = symbols;
    }

    public uint Id { get; }

    public nint Handle { get; }

    /// <summary>Whether ntdll's loader breakpoint, raised once when a process starts under a debugger, has passed.</summary>
    public bool StartupBreakpointSeen { get; set; }

    public IReadOnlyDictionary<uint, nint> Threads => _threads;

    public void AddThread(uint id, nint handle)
    {
        _threads[id] = handle;
    }

    public void RemoveThread(uint id)
    {
        _threads.Remove(id);
    }

    public void AddModule(LoadedModule module)
    {
        _modules.Add(module);
        _symbols.Load(module);
    }

    public LoadedModule? RemoveModule(ulong baseAddress)
    {
        var module = _modules.Remove(baseAddress);

        if (module is not null)
        {
            _symbols.Unload(module);
        }

        return module;
    }

    /// <summary>Takes in the images the loader mapped that this process does not know yet.</summary>
    public void Merge(IEnumerable<LoadedModule> mapped)
    {
        foreach (var image in mapped)
        {
            if (_modules.Find(image.Base) is null)
            {
                AddModule(image);
            }
        }
    }

    public LoadedModule? ModuleAt(ulong address)
    {
        return _modules.Find(address);
    }

    /// <summary>
    /// Where an address lies: <c>ntdll.dll+0x9f1a2 ntdll!RtlFillMemory+0x12</c>; for an image the loader mapped the
    /// offset is the RVA: <c>game.exe RVA 0x1234 (mapped)</c>.
    /// </summary>
    public string Describe(ulong address)
    {
        var module = _modules.Find(address);

        if (module is null)
        {
            return "?";
        }

        _symbols.Locate(module);
        string place = $"{module.Name}+0x{address - module.Base:x}";

        if (module.MappedByLoader)
        {
            place = $"{module.Name} RVA 0x{address - module.Base:x} (mapped)";
        }

        string? symbol = _symbols.Name(address);

        if (symbol is null)
        {
            return place;
        }

        return $"{place} {Path.GetFileNameWithoutExtension(module.Name)}!{symbol}";
    }

    public void Dispose()
    {
        _symbols.Dispose();
    }
}
