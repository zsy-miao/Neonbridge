// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Dxil.Ir;

namespace FiveStar.Shaders.Dxil.Lowering;

/// <summary>A function being inlined (or the entry): where <c>return</c> goes and the slots that carry its value.</summary>
public sealed record FunctionFrame(BasicBlock Exit, HlslType ReturnType, IReadOnlyList<int> ReturnSlots);
