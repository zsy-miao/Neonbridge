// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Ttpf;

/// <summary>Where one mip of a single-slice chain is stored.</summary>
/// <param name="BlockWidth">Width in elements.</param>
/// <param name="BlockHeight">Height in elements.</param>
/// <param name="UnitOffset">Offset of the mip's unit in the slice: the tail block (at 0) for a tail mip, else the mip.</param>
/// <param name="UnitSize">Bytes of the unit: the whole tail block, or the mip padded to whole tiles.</param>
/// <param name="InTail">Whether the mip shares the tail block.</param>
/// <param name="TailOffset">Byte offset of a tail mip's slot inside the tail block (0 otherwise).</param>
public sealed record MipPlacement(int BlockWidth, int BlockHeight, long UnitOffset, long UnitSize, bool InTail, long TailOffset);
