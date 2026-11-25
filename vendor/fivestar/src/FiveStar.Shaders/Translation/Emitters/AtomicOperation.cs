// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Translation.Core;

namespace FiveStar.Shaders.Translation.Emitters;

/// <summary>One atomic being translated: its kind (add, smin, cmpswap, ...), HLSL function, data operand and result name.</summary>
internal sealed record AtomicOperation(string Kind, string? Function, bool Signed, Operand Data, string Result);
