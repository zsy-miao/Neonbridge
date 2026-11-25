// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Loading;

namespace FiveStar.Loader.Host;

/// <summary>Prints the loaded images, the import resolution table per image and module, the totals and the exports.</summary>
internal static class LoadReport
{
    private const int ModuleColumnWidth = 48;
    private const int SymbolColumnWidth = 44;
    private const int OrdinalColumnWidth = 4;
    private const string RvaFormat = "x6";
    private const char DetailSeparator = '+';

    public static void Print(LoaderRuntime runtime, IReadOnlyList<LoadedImage> targets, bool listExports)
    {
        var log = runtime.Log;
        PrintImages(log, runtime.Images);
        PrintImports(log, runtime.Images);
        PrintTotals(log, runtime);

        if (!listExports)
        {
            return;
        }

        foreach (var image in targets)
        {
            PrintExports(log, image);
        }
    }

    private static void PrintImages(LoaderLog log, ImageTable images)
    {
        log.Info(string.Empty);
        log.Info("== loaded images (load order)");

        foreach (var image in images.All)
        {
            string tls = image.TlsHost != 0 ? $", TLS slot {image.TlsIndex} ({image.TlsCallbackCount} cb)" : string.Empty;
            string entry = image.EntryCalled ? image.EntryResult.ToString(System.Globalization.CultureInfo.InvariantCulture) : "not called";
            log.Info($"  {image}, .pdata {image.FunctionCount}{tls}, DllMain={entry}");
        }
    }

    private static void PrintImports(LoaderLog log, ImageTable images)
    {
        log.Info(string.Empty);
        log.Info("== import resolution table (image <- imported module: source x count)");

        foreach (var image in images.All)
        {
            log.Info($"  {image.Name}");

            foreach (var module in image.Imports.GroupBy(i => i.Module, StringComparer.OrdinalIgnoreCase))
            {
                PrintModule(log, module);
            }
        }
    }

    private static void PrintModule(LoaderLog log, IGrouping<string, ImportRecord> module)
    {
        var parts = module.GroupBy(i => i.Source).Select(s => $"{Lower(s.Key)} x{s.Count()}");
        string where = string.Empty;

        if (module.First().Source == ResolutionSource.System)
        {
            where = "  [" + string.Join(", ", module.Select(i => i.Detail.Split(DetailSeparator)[0]).Distinct()) + "]";
        }

        log.Info($"    {module.Key.PadRight(ModuleColumnWidth)} {string.Join(", ", parts)}{where}");

        foreach (var import in module.Where(i => i.Source is ResolutionSource.Shim or ResolutionSource.Unresolved))
        {
            log.Info($"        {import.Symbol.PadRight(SymbolColumnWidth)} -> {Lower(import.Source)}: {import.Detail}");
        }
    }

    private static void PrintTotals(LoaderLog log, LoaderRuntime runtime)
    {
        var all = runtime.Images.All.SelectMany(i => i.Imports).ToList();
        var counts = Enum.GetValues<ResolutionSource>().Select(s => $"{Lower(s)}={all.Count(i => i.Source == s)}");
        log.Info(string.Empty);
        log.Info($"== totals: {all.Count} imports; " + string.Join(", ", counts));
        var unresolved = all.Where(i => i.Source == ResolutionSource.Unresolved).ToList();

        if (unresolved.Count > 0)
        {
            log.Warn("unresolved: " + string.Join(", ", unresolved.Select(i => $"{i.Module}!{i.Symbol}").Distinct()));
        }

        log.Info($"   system modules loaded via LoadLibrary: {runtime.Loader.SystemModules.Count}");
    }

    private static void PrintExports(LoaderLog log, LoadedImage image)
    {
        var exports = image.Exports.Entries();
        log.Info(string.Empty);
        log.Info($"== exports of {image.Name} ({exports.Count})");

        foreach (var export in exports)
        {
            string ordinal = export.Ordinal.ToString(System.Globalization.CultureInfo.InvariantCulture).PadLeft(OrdinalColumnWidth);
            string forwarder = export.Forwarder is not null ? " -> " + export.Forwarder : string.Empty;
            log.Info($"  {ordinal} +0x{export.Rva.ToString(RvaFormat, System.Globalization.CultureInfo.InvariantCulture)} {export.Name ?? "(by ordinal)"}{forwarder}");
        }
    }

    private static string Lower(ResolutionSource source)
    {
        return source.ToString().ToLowerInvariant();
    }
}
