// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>An XGameSaveUpdateHandle: blob writes and deletes staged in memory until the update is submitted.</summary>
internal sealed class SaveUpdate
{
    public SaveUpdate(SaveContainer container, string? displayName)
    {
        Container = container;
        DisplayName = displayName;
    }

    public SaveContainer Container { get; }

    public string? DisplayName { get; }

    public List<SaveBlobData> Operations { get; } = [];
}
