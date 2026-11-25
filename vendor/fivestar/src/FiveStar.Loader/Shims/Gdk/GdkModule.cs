// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>Names shared by the XGameRuntime (GDK) shims.</summary>
internal static class GdkModule
{
    /// <summary>The Xbox-only module every GDK function is imported from.</summary>
    public const string Name = "xgameruntime.dll";

    /// <summary>API-set names the shim also answers for symbols starting with "X".</summary>
    public const string AliasPattern = @"^(api|ext)-ms-win-.*gaming.*\.dll$";

    public const string AliasSymbolPrefix = "X";

    // C bool and BOOL results.
    public const byte True = 1;
    public const byte False = 0;
}
