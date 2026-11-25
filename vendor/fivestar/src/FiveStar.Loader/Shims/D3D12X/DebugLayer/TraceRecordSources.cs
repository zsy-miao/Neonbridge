// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X.DebugLayer;

/// <summary>
/// What <see cref="TraceRecords"/> looks up: the address-map state of a console address, game memory at an address
/// (0 when unreadable), the game's own heap record behind a window offset (0 without the game's heap), and what the
/// texture registry knows of a T#'s base (how its texture got its texels), and the desktop view at a heap index (what a
/// record holding an index names: resource, size, format).
/// </summary>
internal sealed record TraceRecordSources(Func<ulong, string> MapState, Func<ulong, int, nint> Memory, Func<uint, nint> GameRecord, Func<ulong, string> Texture, Func<uint, string> View);
