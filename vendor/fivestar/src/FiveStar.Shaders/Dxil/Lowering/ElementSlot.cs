// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Dxil.Container;

namespace FiveStar.Shaders.Dxil.Lowering;

/// <summary>A signature element inside an entry parameter: its first flattened component there and its scalar kind.</summary>
public readonly record struct ElementSlot(DxilSignatureElement Element, int First, ScalarKind Kind);
