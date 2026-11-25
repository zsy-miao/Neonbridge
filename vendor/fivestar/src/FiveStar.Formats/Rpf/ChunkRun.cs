// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Rpf;

/// <summary>An entry read as a run of chunks up to a known inflated size.</summary>
/// <param name="Start">Archive offset of the first chunk.</param>
/// <param name="Data">The inflated entry; shorter than requested when the run ends early.</param>
/// <param name="End">Archive offset after the last chunk read.</param>
public sealed record ChunkRun(long Start, byte[] Data, long End);
