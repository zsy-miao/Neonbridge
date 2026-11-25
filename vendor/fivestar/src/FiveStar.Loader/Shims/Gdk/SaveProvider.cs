// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>An XGameSaveProviderHandle: a configuration id and the folder its containers live in.</summary>
internal sealed record SaveProvider(string Root, string ConfigId);
