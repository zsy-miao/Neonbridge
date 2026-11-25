// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using FiveStar.Loader.Loading;
using FiveStar.Loader.Shims;

namespace FiveStar.Loader.Host;

/// <summary>
/// Classifies an import the way <see cref="ImportResolver"/> would, without mapping any image: the same
/// <see cref="ShimTable"/> lookups first, then facts about the module (<see cref="HostModules"/>): a package module
/// the loader maps, a PC host module, or a module found nowhere. This is the resolution the <c>fivestar exe analyze</c>
/// report reads, so the import census and the loader never keep two copies of the list.
/// </summary>
internal sealed class ImportClassifier
{
    private const string PackageDetail = "in the package folder; mapped by the loader";
    private const string HostDetail = "no loader shim; resolved from the PC host (System32)";
    private const string MissingDetail = "not in the package, not on this PC and no shim: an Xbox system module, unresolved";

    private readonly ShimTable _shims;
    private readonly IReadOnlyList<string> _packageDirectories;
    private readonly IReadOnlySet<string> _manualModules;

    /// <param name="shims">The loader's shim table.</param>
    /// <param name="packageDirectories">The loader's search directories (the game's package folder).</param>
    /// <param name="manualModules">Modules the loader maps from the package even when the PC host has them.</param>
    public ImportClassifier(ShimTable shims, IReadOnlyList<string> packageDirectories, IReadOnlySet<string> manualModules)
    {
        _shims = shims;
        _packageDirectories = packageDirectories;
        _manualModules = manualModules;
    }

    public ImportClassification Classify(string module, string? name, uint ordinal)
    {
        var entry = _shims.TryResolve(module, name, ordinal);

        if (entry is not null)
        {
            return new ImportClassification(ImportClassKind.Shim, $"{entry.DeclaringType}.{entry.MethodName}");
        }

        var fallback = _shims.TryFallback(module, name, ordinal);

        if (fallback is not null)
        {
            return Stub(fallback);
        }

        return ClassifyModule(ModuleNames.NormalizeImport(module));
    }

    private static ImportClassification Stub(ShimFallback fallback)
    {
        var value = fallback.ReturnValue.ToString("x", CultureInfo.InvariantCulture);

        if (fallback.NotImplemented)
        {
            return new ImportClassification(ImportClassKind.XboxStub, $"not implemented, returns 0x{value}");
        }

        return new ImportClassification(ImportClassKind.XboxStub, $"Xbox-only module default, returns 0x{value}");
    }

    /// <summary>The same order as the resolver: a manual or host-less package module, then the host, then nothing.</summary>
    private ImportClassification ClassifyModule(string key)
    {
        bool onHost = HostModules.IsPresent(key);
        bool inPackage = HostModules.FindInDirectories(_packageDirectories, key) is not null;

        if (inPackage && (_manualModules.Contains(key) || !onHost))
        {
            return new ImportClassification(ImportClassKind.Package, PackageDetail);
        }

        if (onHost)
        {
            return new ImportClassification(ImportClassKind.Host, HostDetail);
        }

        return new ImportClassification(ImportClassKind.Missing, MissingDetail);
    }
}
