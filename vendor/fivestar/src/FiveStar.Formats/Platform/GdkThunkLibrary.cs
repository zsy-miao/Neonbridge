// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Platform;

/// <summary>
/// Reads the named thunks of the GDK's <c>xgameruntime.lib</c>: every function whose code has a REL32 relocation to
/// <c>QueryApiImpl</c>. The GUIDs come from the <c>_GUID_xxxxxxxx_xxxx_...</c> symbols its lea instructions refer to.
/// </summary>
public static class GdkThunkLibrary
{
    private const ushort Rel32 = 4;
    private const string QueryFunction = "QueryApiImpl";
    private const string GuidPrefix = "_GUID_";

    public static IReadOnlyList<GdkThunk> Read(byte[] library)
    {
        var thunks = new List<GdkThunk>();

        foreach (var member in CoffArchive.Members(library))
        {
            var coff = CoffObject.TryRead(member);

            if (coff is null)
            {
                continue;
            }

            for (var index = 0; index < coff.Sections.Count; index++)
            {
                thunks.AddRange(SectionThunks(coff, index));
            }
        }

        return thunks;
    }

    private static IEnumerable<GdkThunk> SectionThunks(CoffObject coff, int index)
    {
        var section = coff.Sections[index];
        var functions = coff.Symbols
            .OfType<CoffSymbol>()
            .Where(symbol => symbol.IsFunction && symbol.Section == index + 1)
            .OrderBy(symbol => symbol.Value)
            .ToList();

        for (var at = 0; at < functions.Count; at++)
        {
            var end = section.Data.Length;

            if (at + 1 < functions.Count)
            {
                end = (int)functions[at + 1].Value;
            }

            var thunk = Thunk(section, functions[at], end);

            if (thunk is not null)
            {
                yield return thunk;
            }
        }
    }

    private static GdkThunk? Thunk(CoffSection section, CoffSymbol function, int end)
    {
        var start = (int)function.Value;
        var relocations = section.Relocations.Where(relocation => relocation.Offset >= start && relocation.Offset < end && relocation.Type == Rel32).ToList();
        var query = relocations.FirstOrDefault(relocation => relocation.Symbol?.Name == QueryFunction);

        if (query is null)
        {
            return null;
        }

        var code = section.Data.Span;
        Guid? clsid = null;
        Guid? iid = null;

        foreach (var relocation in relocations.Where(relocation => relocation.Offset < query.Offset))
        {
            var lea = (int)relocation.Offset - GdkThunkCode.LeaDisplacement;
            var guid = GuidFromSymbol(relocation.Symbol);

            if (guid is null)
            {
                continue;
            }

            switch (GdkThunkCode.LeaTarget(code, lea))
            {
                case GdkRegister.Rcx:
                    clsid = guid;
                    break;
                case GdkRegister.Rdx:
                    iid = guid;
                    break;
            }
        }

        if (iid is null && GdkThunkCode.CopiesRcxToRdx(code, start, (int)query.Offset))
        {
            iid = clsid;
        }

        var slot = GdkThunkCode.FirstApiSlot(code, (int)query.Offset, end);

        if (clsid is null || iid is null || slot is null)
        {
            return null;
        }

        return new GdkThunk(clsid.Value, iid.Value, slot.Value, function.Name);
    }

    private static Guid? GuidFromSymbol(CoffSymbol? symbol)
    {
        if (symbol is null || !symbol.Name.StartsWith(GuidPrefix, StringComparison.Ordinal))
        {
            return null;
        }

        var text = symbol.Name[GuidPrefix.Length..].Replace('_', '-');

        if (!Guid.TryParse(text, out var guid))
        {
            return null;
        }

        return guid;
    }
}
