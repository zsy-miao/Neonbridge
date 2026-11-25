// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>The title and package identity read from MicrosoftGame.config.</summary>
internal sealed record PackageIdentity(
    uint TitleId,
    string StoreId,
    string Name,
    string Publisher,
    Version Version,
    string DisplayName,
    string PublisherDisplayName,
    string Description,
    ulong PersistentLocalStorageBytes)
{
    /// <summary>The identity used when no MicrosoftGame.config can be read.</summary>
    public static PackageIdentity Default { get; } = new(0, string.Empty, string.Empty, string.Empty, new Version(1, 0, 0, 0), string.Empty, string.Empty, string.Empty, 0);
}
