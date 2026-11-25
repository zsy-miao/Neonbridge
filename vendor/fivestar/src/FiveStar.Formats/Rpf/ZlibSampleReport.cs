// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Formats.Common;

namespace FiveStar.Formats.Rpf;

/// <summary>What sampling an archive found: the share of sampled bytes inside complete zlib streams and their kinds.</summary>
/// <param name="CoveredFraction">Sampled bytes covered by complete streams, 0 to 1.</param>
/// <param name="Kinds">Streams per kind of their inflated bytes.</param>
public sealed record ZlibSampleReport(double CoveredFraction, Tally<BlobKind> Kinds);
