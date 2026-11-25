// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Formats.Platform;

namespace FiveStar.Cli.Exe;

/// <summary>Joins the imported functions and their call-site counts with the loader's verdicts.</summary>
internal static class ImportAudit
{
    private const string Unclassified = "Unclassified";
    private const string UnclassifiedDetail = "fivestar-loader was not run";

    /// <summary>Every imported function as an <see cref="ImportGap"/>, in call-site order (most-called first).</summary>
    public static IReadOnlyList<ImportGap> Build(IReadOnlyList<ImportCallSite> sites, IReadOnlyDictionary<string, LoaderVerdict>? verdicts)
    {
        var gaps = new List<ImportGap>();

        foreach (var site in sites)
        {
            gaps.Add(ToGap(site, verdicts));
        }

        gaps.Sort(ByCallSites);

        return gaps;
    }

    /// <summary>The distinct queries to classify: one <c>module!symbol</c> per imported function.</summary>
    public static IReadOnlyCollection<string> Queries(IReadOnlyList<ImportCallSite> sites)
    {
        var queries = new HashSet<string>(StringComparer.Ordinal);

        foreach (var site in sites)
        {
            queries.Add(ImportQuery.Key(site.Dll, site.Function));
        }

        return queries;
    }

    private static ImportGap ToGap(ImportCallSite site, IReadOnlyDictionary<string, LoaderVerdict>? verdicts)
    {
        var kind = Unclassified;
        var detail = UnclassifiedDetail;

        if (verdicts is not null && verdicts.TryGetValue(ImportQuery.Key(site.Dll, site.Function), out var verdict))
        {
            kind = verdict.Kind;
            detail = verdict.Detail;
        }

        return new ImportGap(site.Dll, site.Function, site.Delayed, kind, detail, site.Sites);
    }

    private static int ByCallSites(ImportGap left, ImportGap right)
    {
        if (left.Sites != right.Sites)
        {
            return right.Sites.CompareTo(left.Sites);
        }

        return string.CompareOrdinal(left.Dll + left.Function, right.Dll + right.Function);
    }
}
