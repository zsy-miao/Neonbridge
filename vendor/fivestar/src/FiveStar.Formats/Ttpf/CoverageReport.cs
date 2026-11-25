// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Formats.Common;

namespace FiveStar.Formats.Ttpf;

/// <summary>How many textures of a page file are readable, by status, with their span bytes.</summary>
/// <param name="Textures">Textures per status name.</param>
/// <param name="SpanBytes">Span bytes per status name.</param>
/// <param name="Blockers">BCPack-blocked textures per (codec, format name).</param>
public sealed record CoverageReport(Tally<string> Textures, Tally<string> SpanBytes, Tally<(int Codec, string Format)> Blockers);
