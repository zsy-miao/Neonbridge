// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Host;

/// <summary>The loader's verdict on one import: how it would resolve, and a human-readable reason.</summary>
/// <param name="Kind">The resolution class.</param>
/// <param name="Detail">The shim behind it, the stub's return value, or why nothing in the loader answers.</param>
internal sealed record ImportClassification(ImportClassKind Kind, string Detail);
