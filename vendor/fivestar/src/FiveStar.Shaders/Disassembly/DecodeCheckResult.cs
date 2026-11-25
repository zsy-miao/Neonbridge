// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Disassembly;

/// <summary>What <see cref="DecoderCheck"/> compared in one program and where the listings differ.</summary>
public sealed record DecodeCheckResult(int Blocks, int Instructions, IReadOnlyList<DecodeMismatch> Mismatches);
