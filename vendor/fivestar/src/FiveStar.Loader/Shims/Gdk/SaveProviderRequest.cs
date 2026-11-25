// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>The arguments of XGameSaveInitializeProviderAsync: the requesting user and the configuration id.</summary>
internal sealed record SaveProviderRequest(nint User, string? ConfigId);
