// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X;

/// <summary>Consecutive 64 KiB pages of a game range that sit back to back in one page section.</summary>
/// <param name="Chunk">The section; null for pages nothing is mapped at.</param>
/// <param name="Offset">Byte offset of the first page in the section.</param>
/// <param name="Start">Byte offset of the run in the game range.</param>
/// <param name="Length">Bytes in the run.</param>
internal readonly record struct PageRun(XMemPageChunk? Chunk, ulong Offset, ulong Start, ulong Length);
