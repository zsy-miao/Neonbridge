// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Compilation;

/// <summary>
/// A shader cache split by owner: entries the current identity uses, entries of older builds (no current use) with
/// left-over temporary files, and the usage records of other identities.
/// </summary>
public sealed record CacheSweep(int CurrentEntries, long CurrentBytes, int StaleEntries, long StaleBytes, int OtherRecords);
