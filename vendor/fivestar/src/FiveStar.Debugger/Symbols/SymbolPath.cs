// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Debugger.Symbols;

/// <summary>
/// A symbol path split into the folders dbghelp searches itself and the <c>srv*</c> stores the debugger resolves:
/// System32's dbghelp has no symsrv.dll, so the debugger downloads from the server and adds each PDB's folder.
/// </summary>
internal sealed record SymbolPath(IReadOnlyList<string> Folders, IReadOnlyList<SymbolStore> Stores)
{
    private const char ElementSeparator = ';';
    private const char FieldSeparator = '*';
    private const string StorePrefix = "srv*";
    private const string CachePrefix = "cache*";
    private const string HttpScheme = "http";
    /// <summary>srv, cache, server.</summary>
    private const int StoreWithCache = 3;

    /// <summary>Parses <c>dir;srv*cache*https://server;srv*https://server</c>; a store without a cache uses <paramref name="defaultCache"/>.</summary>
    public static SymbolPath Parse(string? text, string defaultCache)
    {
        var folders = new List<string>();
        var stores = new List<SymbolStore>();

        foreach (string element in (text ?? string.Empty).Split(ElementSeparator, StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries))
        {
            if (element.StartsWith(CachePrefix, StringComparison.OrdinalIgnoreCase))
            {
                continue;
            }

            if (!element.StartsWith(StorePrefix, StringComparison.OrdinalIgnoreCase))
            {
                folders.Add(element);

                continue;
            }

            stores.Add(ParseStore(element.Split(FieldSeparator), defaultCache));
        }

        return new SymbolPath(folders, stores);
    }

    private static SymbolStore ParseStore(string[] fields, string defaultCache)
    {
        string? cache = null;
        string last = fields[^1];

        if (fields.Length >= StoreWithCache && fields[1].Length > 0)
        {
            cache = fields[1];
        }

        if (!last.StartsWith(HttpScheme, StringComparison.OrdinalIgnoreCase))
        {
            return new SymbolStore(cache ?? last, null);
        }

        return new SymbolStore(cache ?? defaultCache, last);
    }
}
