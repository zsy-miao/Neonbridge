// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Text.RegularExpressions;
using FiveStar.Loader.Codegen;
using FiveStar.Loader.Loading;

namespace FiveStar.Loader.Shims;

/// <summary>
/// Every shim of this assembly: [Shim] methods, [ShimModule] (Xbox-only) modules with their API-set aliases, and
/// [NotImplementedTable] catalogs, registered by the code FiveStar.Loader.Generators writes from those attributes
/// (<see cref="ShimTypes"/>; no reflection, so trimming and NativeAOT keep them). Looked up by (normalized module,
/// symbol), then through an alias, then under "*".
/// </summary>
internal sealed class ShimTable
{
    /// <summary>Microsoft x64 ABI: the first four arguments travel in rcx, rdx, r8 and r9.</summary>
    private const int RegisterArguments = 4;

    private readonly Dictionary<(string Module, string Symbol), ShimEntry> _entries = [];
    private readonly HashSet<string> _exclusiveModules = new(StringComparer.OrdinalIgnoreCase);
    private readonly Dictionary<string, ShimModuleAttribute> _moduleInfo = new(StringComparer.OrdinalIgnoreCase);
    private readonly Dictionary<(string Module, string Symbol), long> _notImplemented = [];
    private readonly List<ShimAlias> _aliases = [];
    private readonly LastErrorBridge _lastError;

    public ShimTable(LastErrorBridge lastError, CodeHeap heap)
    {
        _lastError = lastError;

        ShimTypes.Register(this);

        // The last-error wrappers become executable.
        heap.Seal();
    }

    public IReadOnlyCollection<ShimEntry> All => _entries.Values;

    /// <summary>Known but unimplemented functions: (module, symbol) to the value their stub returns.</summary>
    public IReadOnlyDictionary<(string Module, string Symbol), long> NotImplementedFunctions => _notImplemented;

    public bool IsExclusiveModule(string module)
    {
        return _exclusiveModules.Contains(ModuleNames.NormalizeImport(module));
    }

    /// <summary>The module that answers for <paramref name="symbol"/> of <paramref name="module"/>: an alias target, or the module itself.</summary>
    public string EffectiveModule(string module, string? symbol)
    {
        string key = ModuleNames.NormalizeImport(module);

        if (symbol is null)
        {
            return key;
        }

        foreach (var alias in _aliases)
        {
            if (alias.Pattern.IsMatch(key) && symbol.StartsWith(alias.Prefix, StringComparison.Ordinal))
            {
                return alias.Target;
            }
        }

        return key;
    }

    public ShimEntry? TryResolve(string module, string? name, uint ordinal)
    {
        string symbol = ModuleNames.SymbolKey(name, ordinal);
        string key = ModuleNames.NormalizeImport(module);

        if (_entries.TryGetValue((key, symbol), out var direct))
        {
            return direct;
        }

        string effective = EffectiveModule(module, name);

        if (effective != key && _entries.TryGetValue((effective, symbol), out var aliased))
        {
            return aliased;
        }

        return _entries.GetValueOrDefault((ModuleNames.AnyModule, symbol));
    }

    /// <summary>
    /// The second chance after <see cref="TryResolve"/>: a known unimplemented function, or any other symbol of an
    /// Xbox-only module (which returns the module's unresolved value). Null for everything else.
    /// </summary>
    public ShimFallback? TryFallback(string module, string? name, uint ordinal)
    {
        string effective = EffectiveModule(module, name);

        if (_notImplemented.TryGetValue((effective, ModuleNames.SymbolKey(name, ordinal)), out long value))
        {
            return new ShimFallback(value, true);
        }

        if (!_exclusiveModules.Contains(effective))
        {
            return null;
        }

        long unresolved = 0;

        if (_moduleInfo.TryGetValue(effective, out var info))
        {
            unresolved = info.UnresolvedReturn;
        }

        return new ShimFallback(unresolved, false);
    }

    /// <summary>Removes the shim "module!symbol", to exercise the unresolved-import path.</summary>
    public bool Disable(string specification)
    {
        int bang = specification.IndexOf('!', StringComparison.Ordinal);

        if (bang < 0)
        {
            return false;
        }

        return _entries.Remove((ModuleNames.NormalizeImport(specification[..bang]), specification[(bang + 1)..]));
    }

    /// <summary>Declares an Xbox-only module (a [ShimModule]); called by the generated RegisterShims.</summary>
    public void AddModule(ShimModuleAttribute module)
    {
        string key = ModuleNames.NormalizeImport(module.Module);
        _exclusiveModules.Add(key);
        _moduleInfo[key] = module;

        if (module.AliasPattern is not null)
        {
            _aliases.Add(new ShimAlias(new Regex(module.AliasPattern, RegexOptions.IgnoreCase), module.AliasSymbolPrefix, key));
        }
    }

    /// <summary>Adds a [NotImplementedTable] catalog; called by the generated RegisterShims.</summary>
    public void AddNotImplemented(string module, (string Name, long ReturnValue)[] catalog)
    {
        foreach (var (name, value) in catalog)
        {
            _notImplemented[(ModuleNames.NormalizeImport(module), name)] = value;
        }
    }

    /// <summary>
    /// Adds a [Shim] method under every (module, name) it answers; called by the generated RegisterShims with the
    /// method's native entry point (the address of an [UnmanagedCallersOnly] method).
    /// </summary>
    public void AddMethod(nint address, int parameterCount, bool setsLastError, string type, string method, params ReadOnlySpan<ShimBinding> bindings)
    {
        if (setsLastError)
        {
            int stackArguments = Math.Max(0, parameterCount - RegisterArguments);
            address = _lastError.Wrap(address, stackArguments);
        }

        foreach (var binding in bindings)
        {
            var key = (ModuleNames.NormalizeImport(binding.Module), binding.Name);
            _entries[key] = new ShimEntry(key.Item1, key.Item2, address, type, method);
        }
    }
}
