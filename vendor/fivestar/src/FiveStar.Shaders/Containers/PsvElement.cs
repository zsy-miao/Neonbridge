// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Containers;

/// <summary>
/// One signature element of a PSV0 chunk, in declaration order. <see cref="Name"/> is empty for system values, whose
/// DXIL semantic kind is <see cref="SemanticKind"/>; <see cref="ComponentType"/> is the DXIL signature component type
/// (1 uint, 2 int, 3 float, 4..6 the 16-bit forms); <see cref="InterpolationMode"/> is the DXIL interpolation mode.
/// </summary>
public sealed record PsvElement(
    string Name,
    int SemanticIndex,
    int Rows,
    int StartRow,
    int Columns,
    int StartColumn,
    bool Allocated,
    PsvSemanticKind SemanticKind,
    int ComponentType,
    int InterpolationMode);
