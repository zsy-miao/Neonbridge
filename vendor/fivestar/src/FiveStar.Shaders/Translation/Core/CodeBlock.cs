// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Translation.Core;

/// <summary>
/// A translated basic block: the address of its first instruction, its HLSL statements (the last one sets <c>pc</c>)
/// and how it ends.
/// </summary>
internal sealed record CodeBlock(int Address, List<string> Lines, BlockExit Exit);
