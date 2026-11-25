// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Cli.Exe;

/// <summary>The <c>module!symbol</c> key the report sends to the loader and looks verdicts up by.</summary>
internal static class ImportQuery
{
    private const char Separator = '!';

    public static string Key(string dll, string function)
    {
        return dll.ToLowerInvariant() + Separator + function;
    }
}
