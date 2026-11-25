// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Formats.Platform;

namespace FiveStar.Cli.Exe;

/// <summary>
/// Finds the GDK thunks the game calls and counts the direct call sites of each; the user's PC
/// <c>xgameruntime.lib</c>, when given, names them. Reuses <see cref="GdkThunkScanner"/> and
/// <see cref="GdkThunkLibrary"/>; it does not re-derive the thunk layout.
/// </summary>
internal static class GdkAudit
{
    // The Xbox-only module every GDK function is imported from (see docs/loader.md, the thunk ABI).
    private const string GdkModule = "xgameruntime.dll";
    private const string ShimKind = "Shim";

    public static IReadOnlyList<GdkThunkTally> Scan(PeImage image, byte[]? libraryBytes)
    {
        var names = new Dictionary<(Guid, Guid, int), string?>();

        if (libraryBytes is not null)
        {
            names = NamesByKey(GdkThunkLibrary.Read(libraryBytes));
        }

        var thunks = new List<(GdkThunk Thunk, uint Function)>();

        foreach (var site in GdkThunkScanner.ScanSites(image))
        {
            thunks.Add((site.Thunk, PeFunctionTable.FunctionStart(image, site.Rva) ?? site.Rva));
        }

        var callers = DirectCallCounter.Count(image, thunks.Select(thunk => thunk.Function).ToHashSet());
        var tallies = new List<GdkThunkTally>();

        foreach (var group in thunks.GroupBy(thunk => Key(thunk.Thunk)))
        {
            var functions = group.Select(thunk => thunk.Function).Distinct().ToList();
            tallies.Add(ToTally(group.Key, names, functions.Min(), functions.Sum(function => callers[function])));
        }

        tallies.Sort(BySites);

        return tallies;
    }

    /// <summary>The <c>xgameruntime.dll!Name</c> queries for the named thunks, so the loader classifies them too.</summary>
    public static IReadOnlyCollection<string> NameQueries(IReadOnlyList<GdkThunkTally> tallies)
    {
        var queries = new HashSet<string>(StringComparer.Ordinal);

        foreach (var tally in tallies)
        {
            if (tally.Name is not null)
            {
                queries.Add(ImportQuery.Key(GdkModule, tally.Name));
            }
        }

        return queries;
    }

    public static IReadOnlyList<GdkCall> ToCalls(IReadOnlyList<GdkThunkTally> tallies, IReadOnlyDictionary<string, LoaderVerdict>? verdicts)
    {
        var calls = new List<GdkCall>();

        foreach (var tally in tallies)
        {
            var shimmed = Shimmed(tally, verdicts);
            calls.Add(new GdkCall(tally.Name, tally.Clsid.ToString(), tally.Iid.ToString(), tally.Slot, tally.Thunk, tally.Sites, shimmed));
        }

        return calls;
    }

    private static bool? Shimmed(GdkThunkTally tally, IReadOnlyDictionary<string, LoaderVerdict>? verdicts)
    {
        if (tally.Name is null || verdicts is null)
        {
            return null;
        }

        if (!verdicts.TryGetValue(ImportQuery.Key(GdkModule, tally.Name), out var verdict))
        {
            return false;
        }

        return string.Equals(verdict.Kind, ShimKind, StringComparison.Ordinal);
    }

    private static GdkThunkTally ToTally((Guid Clsid, Guid Iid, int Slot) key, IReadOnlyDictionary<(Guid, Guid, int), string?> names, uint thunk, int sites)
    {
        var name = names.GetValueOrDefault((key.Clsid, key.Iid, key.Slot));

        return new GdkThunkTally(name, key.Clsid, key.Iid, key.Slot, thunk, sites);
    }

    private static Dictionary<(Guid, Guid, int), string?> NamesByKey(IReadOnlyList<GdkThunk> library)
    {
        var names = new Dictionary<(Guid, Guid, int), string?>();

        foreach (var thunk in library)
        {
            names[(thunk.Clsid, thunk.Iid, thunk.Slot)] = thunk.Name;
        }

        return names;
    }

    private static (Guid Clsid, Guid Iid, int Slot) Key(GdkThunk thunk)
    {
        return (thunk.Clsid, thunk.Iid, thunk.Slot);
    }

    private static int BySites(GdkThunkTally left, GdkThunkTally right)
    {
        if (left.Sites != right.Sites)
        {
            return right.Sites.CompareTo(left.Sites);
        }

        return string.CompareOrdinal(left.Name ?? left.Clsid.ToString(), right.Name ?? right.Clsid.ToString());
    }
}
