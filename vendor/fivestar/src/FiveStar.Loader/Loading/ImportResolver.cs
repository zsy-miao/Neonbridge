// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using System.Runtime.InteropServices;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;
using FiveStar.Loader.Pe;
using FiveStar.Loader.Shims;

namespace FiveStar.Loader.Loading;

/// <summary>
/// Binds an image's imports and delay imports (delay imports eagerly), by name and by ordinal. Resolution order:
/// (1) the shim table, (2) an image this loader mapped or maps now (a manual module, or a module of the search
/// directories that the PC host does not have), (3) System32 through LoadLibraryEx. Export forwarders are followed
/// through the same order.
/// </summary>
internal sealed unsafe class ImportResolver
{
    private const int MaxForwarderDepth = 8;
    private const int SymbolColumnWidth = 48;
    private const int SourceColumnWidth = 10;

    private readonly ShimTable _shims;
    private readonly UnresolvedStubs _stubs;
    private readonly LoaderOptions _options;
    private readonly ImageLoader _loader;
    private readonly CallTrace _trace;
    private readonly LoaderLog _log;
    private readonly Dictionary<string, nint> _system = new(StringComparer.OrdinalIgnoreCase);

    public ImportResolver(ShimTable shims, UnresolvedStubs stubs, LoaderOptions options, ImageLoader loader, CallTrace trace, LoaderLog log)
    {
        _shims = shims;
        _stubs = stubs;
        _options = options;
        _loader = loader;
        _trace = trace;
        _log = log;
    }

    /// <summary>System modules loaded for imports, by import key.</summary>
    public IReadOnlyDictionary<string, nint> SystemHandles => _system;

    public void BindImports(LoadedImage image)
    {
        var directory = image.Pe.Directory(DataDirectoryIndex.Import);

        if (!directory.IsPresent)
        {
            return;
        }

        for (byte* descriptor = (byte*)image.Va(directory.Rva); ; descriptor += ImportFormat.DescriptorSize)
        {
            uint lookupRva = *(uint*)(descriptor + ImportFormat.OriginalFirstThunkOffset);
            uint nameRva = *(uint*)(descriptor + ImportFormat.NameOffset);
            uint iatRva = *(uint*)(descriptor + ImportFormat.FirstThunkOffset);

            if (nameRva == 0 && iatRva == 0)
            {
                break;
            }

            uint namesRva = lookupRva != 0 ? lookupRva : iatRva;
            string module = ExportTable.ReadAnsi(image.Va(nameRva));
            BindThunks(image, module, (ulong*)image.Va(namesRva), (ulong*)image.Va(iatRva), false);
        }
    }

    public void BindDelayImports(LoadedImage image)
    {
        var directory = image.Pe.Directory(DataDirectoryIndex.DelayImport);

        if (!directory.IsPresent)
        {
            return;
        }

        uint* descriptor = (uint*)image.Va(directory.Rva);

        for (; descriptor[ImportFormat.DelayDllNameField] != 0; descriptor += ImportFormat.DelayDescriptorFields)
        {
            bool rvaBased = (descriptor[ImportFormat.DelayAttributesField] & ImportFormat.DelayRvaBased) != 0;
            string module = ExportTable.ReadAnsi(DelayAddress(image, rvaBased, descriptor[ImportFormat.DelayDllNameField]));
            var names = (ulong*)DelayAddress(image, rvaBased, descriptor[ImportFormat.DelayNameTableField]);
            var iat = (ulong*)DelayAddress(image, rvaBased, descriptor[ImportFormat.DelayIatField]);
            BindThunks(image, module, names, iat, true);

            if (descriptor[ImportFormat.DelayModuleHandleField] != 0)
            {
                *(nint*)DelayAddress(image, rvaBased, descriptor[ImportFormat.DelayModuleHandleField]) = ModuleHandleFor(module);
            }
        }
    }

    /// <summary>Resolves one import through the shim table, the mapped images and System32, in that order.</summary>
    public ImportRecord Resolve(string module, string? name, uint ordinal, bool delay, int depth)
    {
        string key = ModuleNames.NormalizeImport(module);
        string symbol = ModuleNames.SymbolKey(name, ordinal);

        if (depth > MaxForwarderDepth)
        {
            return Unresolved(module, symbol, delay, "forwarder chain too deep", 0);
        }

        var shim = ResolveShim(module, key, name, ordinal, delay);

        if (shim is not null)
        {
            return shim;
        }

        var mapped = ResolveImage(module, key, name, ordinal, delay, depth);

        if (mapped is not null)
        {
            return mapped;
        }

        return ResolveSystem(module, key, name, ordinal, delay);
    }

    /// <summary>Resolves a forwarder string ("DLL.Func" or "DLL.#N") and reports it under the original name.</summary>
    public ImportRecord Forward(string module, string symbol, string forwarder, bool delay, int depth, string via)
    {
        int dot = forwarder.LastIndexOf('.');
        string targetModule = forwarder[..dot];
        string targetSymbol = forwarder[(dot + 1)..];
        ImportRecord record;

        if (targetSymbol.StartsWith('#'))
        {
            record = Resolve(targetModule, null, uint.Parse(targetSymbol[1..], CultureInfo.InvariantCulture), delay, depth + 1);
        }
        else
        {
            record = Resolve(targetModule, targetSymbol, 0, delay, depth + 1);
        }

        return record with { Module = module, Symbol = symbol, Detail = $"{record.Detail} (forwarded {via} -> {forwarder})" };
    }

    private static nint DelayAddress(LoadedImage image, bool rvaBased, uint value)
    {
        if (rvaBased)
        {
            return image.Va(value);
        }

        return (nint)(value - (long)image.Pe.ImageBase + (long)image.Base);
    }

    private void BindThunks(LoadedImage image, string module, ulong* names, ulong* iat, bool delay)
    {
        for (int i = 0; names[i] != 0; i++)
        {
            ulong thunk = names[i];
            string? symbol = null;
            uint ordinal = 0;

            if ((thunk & ImportFormat.OrdinalFlag) != 0)
            {
                ordinal = (uint)(thunk & ImportFormat.OrdinalMask);
            }
            else
            {
                symbol = ExportTable.ReadAnsi(image.Va((uint)thunk + ImportFormat.HintSize));
            }

            var record = Resolve(module, symbol, ordinal, delay, 0);
            iat[i] = (ulong)TraceTarget(record);
            image.Imports.Add(record);
            string prefix = delay ? "delay " : string.Empty;
            _log.Detail($"    {prefix}{module}!{record.Symbol.PadRight(SymbolColumnWidth)} -> {record.Source.ToString().PadRight(SourceColumnWidth)} {record.Detail}");
        }
    }

    /// <summary>The address the IAT receives: a recording thunk for shims and stubs (and System32) while the call trace is on.</summary>
    private nint TraceTarget(ImportRecord record)
    {
        bool traced = record.Source is ResolutionSource.Shim or ResolutionSource.Unresolved;
        traced |= _trace.IncludesSystem && record.Source == ResolutionSource.System;
        traced |= record.Source == ResolutionSource.Image && _trace.TracedModules.Contains(record.Module);

        if (!_trace.Enabled || !traced)
        {
            return record.Address;
        }

        return _trace.Wrap(record.Address, $"{record.Module}!{record.Symbol}");
    }

    private nint ModuleHandleFor(string module)
    {
        string key = ModuleNames.NormalizeImport(module);
        var image = _loader.FindMapped(key);

        if (image is not null)
        {
            return image.Base;
        }

        return _system.GetValueOrDefault(key);
    }

    private ImportRecord? ResolveShim(string module, string key, string? name, uint ordinal, bool delay)
    {
        string symbol = ModuleNames.SymbolKey(name, ordinal);
        var entry = _shims.TryResolve(key, name, ordinal);

        if (entry is not null)
        {
            return new ImportRecord(module, symbol, ResolutionSource.Shim, $"{entry.DeclaringType}.{entry.MethodName}", entry.Address, delay);
        }

        var fallback = _shims.TryFallback(key, name, ordinal);

        if (fallback is null)
        {
            return null;
        }

        if (!fallback.NotImplemented)
        {
            return Unresolved(module, symbol, delay, "Xbox-only module, no shim for this function", fallback.ReturnValue);
        }

        nint stub = _stubs.Create(module, symbol, fallback.ReturnValue, true, false);

        return new ImportRecord(module, symbol, ResolutionSource.Shim, $"not implemented -> returns 0x{fallback.ReturnValue:x}, logs first call", stub, delay);
    }

    private ImportRecord? ResolveImage(string module, string key, string? name, uint ordinal, bool delay, int depth)
    {
        var target = FindOrMapTarget(key);

        if (target is null)
        {
            return null;
        }

        string symbol = ModuleNames.SymbolKey(name, ordinal);

        // Import cycles are fine: exports are read before an image binds its own imports.
        var export = target.Exports.Find(name, ordinal);

        if (export is null)
        {
            return Unresolved(module, symbol, delay, $"not exported by mapped {target.Name}", 0);
        }

        if (export.Forwarder is not null)
        {
            return Forward(module, symbol, export.Forwarder, delay, depth, $"mapped {target.Name}");
        }

        return new ImportRecord(module, symbol, ResolutionSource.Image, $"{target.Name}+0x{(long)export.Address - (long)target.Base:x}", export.Address, delay);
    }

    /// <summary>A mapped image, or one mapped now: a manual module, or a package module the PC host does not have.</summary>
    private LoadedImage? FindOrMapTarget(string key)
    {
        var mapped = _loader.FindMapped(key);

        if (mapped is not null)
        {
            return mapped;
        }

        bool manual = _options.ManualModules.Contains(key);

        if (!manual && HostModules.IsPresent(key))
        {
            return null;
        }

        string? file = _loader.FindModuleFile(key);

        if (file is null)
        {
            if (manual)
            {
                _log.Warn($"{key} is configured as manual but was not found in search dirs; falling back to System32");
            }

            return null;
        }

        return _loader.Load(file, false);
    }

    /// <summary>System32 only, so a same-named DLL in the game or current directory is never picked up by accident.</summary>
    private ImportRecord ResolveSystem(string module, string key, string? name, uint ordinal, bool delay)
    {
        string symbol = ModuleNames.SymbolKey(name, ordinal);

        if (!_system.TryGetValue(key, out var handle))
        {
            handle = LoadSystemModule(key);

            if (handle == 0)
            {
                return Unresolved(module, symbol, delay, $"LoadLibrary failed (error {Marshal.GetLastPInvokeError()})", 0);
            }
        }

        nint address = SystemModules.Export(handle, name, ordinal);

        if (address == 0)
        {
            return Unresolved(module, symbol, delay, "GetProcAddress failed", 0);
        }

        return new ImportRecord(module, symbol, ResolutionSource.System, SystemModules.Describe(address), address, delay);
    }

    private nint LoadSystemModule(string key)
    {
        nint handle;

        fixed (char* name = key)
        {
            handle = Kernel32.LoadLibraryExW(name, 0, Win32Constants.LoadLibrarySearchSystem32);
        }

        if (handle != 0)
        {
            _system[key] = handle;
            _log.Detail($"  LoadLibrary({key}) = 0x{handle:x} [{SystemModules.Describe(handle)}]");
        }

        return handle;
    }

    private ImportRecord Unresolved(string module, string symbol, bool delay, string why, long returnValue)
    {
        if (_options.Unresolved == UnresolvedMode.Error)
        {
            throw new EntryPointNotFoundException($"unresolved import {module}!{symbol}: {why}");
        }

        string outcome = $"logging stub returning 0x{returnValue:x}";

        if (_options.Unresolved == UnresolvedMode.Trap)
        {
            outcome = "trap";
        }

        _log.Warn($"unresolved import {module}!{symbol}: {why} -> {outcome}");
        nint stub = _stubs.Create(module, symbol, returnValue, false, false);

        return new ImportRecord(module, symbol, ResolutionSource.Unresolved, why, stub, delay);
    }
}
