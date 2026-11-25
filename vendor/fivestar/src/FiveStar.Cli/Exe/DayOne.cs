// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;

namespace FiveStar.Cli.Exe;

/// <summary>Builds the ordered "day one" gap list: what blocks the game from running, biggest first.</summary>
internal static class DayOne
{
    private const string XboxStubKind = "XboxStub";
    private const string MissingKind = "Missing";
    private const int TopNamesShown = 6;

    public static IReadOnlyList<GapItem> Build(
        IReadOnlyList<ImportGap> graphics,
        IReadOnlyList<GdkCall> gdkCalls,
        IReadOnlyList<ImportGap> imports,
        IReadOnlyList<SymbolReference> archiveReferences)
    {
        var gaps = new List<GapItem>();
        AddGraphics(graphics, gaps);
        AddArchives(archiveReferences, gaps);
        AddGdk(gdkCalls, gaps);
        AddImports(imports, gaps);

        return gaps;
    }

    private static void AddGraphics(IReadOnlyList<ImportGap> graphics, List<GapItem> gaps)
    {
        if (graphics.Count == 0)
        {
            return;
        }

        var sites = graphics.Sum(import => import.Sites);
        var why = $"{graphics.Count} graphics imports, {sites} call sites; the loader forwards only root-signature serialization, so command lists are not translated yet";
        gaps.Add(new GapItem("Graphics", "D3D12.X command translation", why));
    }

    private static void AddArchives(IReadOnlyList<SymbolReference> archiveReferences, List<GapItem> gaps)
    {
        var why = $"the RPF9 table of contents and name table are encrypted; {archiveReferences.Count} references locate the archive code so names and entry boundaries can be recovered";
        gaps.Add(new GapItem("Archives", "RPF9 table of contents", why));
    }

    private static void AddGdk(IReadOnlyList<GdkCall> gdkCalls, List<GapItem> gaps)
    {
        var missing = gdkCalls.Where(call => call.Shimmed == false).ToList();

        if (missing.Count == 0)
        {
            return;
        }

        var names = missing.Where(call => call.Name is not null).Take(TopNamesShown).Select(call => call.Name!);
        var why = $"called by the game but no loader shim: {string.Join(", ", names)}";
        gaps.Add(new GapItem("GDK", $"{missing.Count} GDK functions without a shim", why));
    }

    private static void AddImports(IReadOnlyList<ImportGap> imports, List<GapItem> gaps)
    {
        AddMissing(imports, gaps);
        var stubs = imports.Where(import => string.Equals(import.Kind, XboxStubKind, StringComparison.Ordinal)).ToList();

        if (stubs.Count == 0)
        {
            return;
        }

        var top = stubs.Take(TopNamesShown).Select(import => $"{import.Dll}!{import.Function} ({import.Sites.ToString(CultureInfo.InvariantCulture)})");
        var why = $"Xbox-only imports answered by a fixed-value stub; confirm the game tolerates them: {string.Join(", ", top)}";
        gaps.Add(new GapItem("Loader", $"{stubs.Count} Xbox-only imports on stubs", why));
    }

    private static void AddMissing(IReadOnlyList<ImportGap> imports, List<GapItem> gaps)
    {
        var modules = imports.Where(import => string.Equals(import.Kind, MissingKind, StringComparison.Ordinal)).Select(import => import.Dll).Distinct(StringComparer.OrdinalIgnoreCase).ToList();

        if (modules.Count == 0)
        {
            return;
        }

        var why = $"not in the package, not on this PC and no shim, so their imports stay unresolved: {string.Join(", ", modules)}";
        gaps.Add(new GapItem("Loader", $"{modules.Count} unknown Xbox system modules", why));
    }
}
