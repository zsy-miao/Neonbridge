// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>An XGameSaveContainerHandle: a named container of a provider, stored as an escaped folder.</summary>
internal sealed record SaveContainer(SaveProvider Provider, string Name)
{
    public string Directory => Path.Combine(Provider.Root, NameEscaping.Encode(Name));
}
