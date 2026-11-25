// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Debugger.Native;
using FiveStar.Debugger.Session;

namespace FiveStar.Debugger.Symbols;

/// <summary>
/// dbghelp's symbol handler for one debuggee: modules are registered as their load events arrive, without reading the
/// process, and symbols load on the first lookup (exports when there is no PDB).
/// </summary>
internal sealed unsafe class SymbolSession : IDisposable
{
    private const int MaxNameLength = 1024;
    private const char PathSeparator = ';';
    private const string ExportMark = " (export)";

    private const SymbolOptions Options =
        SymbolOptions.UndecoratedNames | SymbolOptions.DeferredLoads | SymbolOptions.FailCriticalErrors | SymbolOptions.NoPrompts;

    private readonly nint _process;
    private readonly SymbolDownloader _downloader;
    private readonly List<string> _searchPath;
    private readonly HashSet<ulong> _located = [];

    public SymbolSession(nint process, IReadOnlyList<string> folders, SymbolDownloader downloader)
    {
        _process = process;
        _downloader = downloader;
        _searchPath = [.. folders];
        _ = DbgHelp.SymSetOptions(Options);

        fixed (char* path = string.Join(PathSeparator, _searchPath))
        {
            _ = DbgHelp.SymInitialize(process, path, 0);
        }
    }

    public void Load(LoadedModule module)
    {
        fixed (char* path = module.Path)
        {
            _ = DbgHelp.SymLoadModuleEx(_process, 0, path, null, module.Base, (uint)module.Size, 0, 0);
        }
    }

    public void Unload(LoadedModule module)
    {
        _ = DbgHelp.SymUnloadModule64(_process, module.Base);
    }

    /// <summary>Looks up the module's PDB in the symbol stores once; a PDB found joins the search path and the module reloads.</summary>
    public void Locate(LoadedModule module)
    {
        if (module.MappedByLoader || !_located.Add(module.Base))
        {
            return;
        }

        string? folder = _downloader.Locate(module.Path);

        if (folder is null || _searchPath.Contains(folder, StringComparer.OrdinalIgnoreCase))
        {
            return;
        }

        _searchPath.Add(folder);

        fixed (char* path = string.Join(PathSeparator, _searchPath))
        {
            _ = DbgHelp.SymSetSearchPath(_process, path);
        }

        Unload(module);
        Load(module);
    }

    /// <summary>
    /// The symbol at the address with its displacement, <c>RtlFillMemory+0x12</c>, marked <c>(export)</c> when it is
    /// only the nearest export; null when there is none.
    /// </summary>
    public string? Name(ulong address)
    {
        int size = sizeof(SymbolInfo) + (MaxNameLength * sizeof(char));
        byte* buffer = stackalloc byte[size];
        new Span<byte>(buffer, size).Clear();
        var symbol = (SymbolInfo*)buffer;
        symbol->SizeOfStruct = (uint)sizeof(SymbolInfo);
        symbol->MaxNameLength = MaxNameLength;

        ulong displacement;

        if (DbgHelp.SymFromAddr(_process, address, &displacement, symbol) == 0)
        {
            return null;
        }

        int length = (int)Math.Min(symbol->NameLength, MaxNameLength - 1);
        string name = new((char*)(buffer + SymbolInfo.NameOffset), 0, length);

        if (displacement != 0)
        {
            name = $"{name}+0x{displacement:x}";
        }

        // Without a PDB dbghelp names the nearest export, which may be a different function far away.
        if ((symbol->Flags & SymbolInfo.ExportFlag) != 0)
        {
            return name + ExportMark;
        }

        return name;
    }

    public void Dispose()
    {
        _ = DbgHelp.SymCleanup(_process);
    }
}
