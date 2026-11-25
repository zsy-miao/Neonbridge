// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Containers;

/// <summary>
/// One element of an ISG1/OSG1/PSG1 signature chunk. <see cref="SystemValue"/> is the D3D system-value name
/// (empty for a plain semantic, the number for an unknown code); <see cref="ComponentType"/> is uint, int, float
/// or a min-precision name; <see cref="Mask"/> lists the used components in xyzw order.
/// </summary>
public sealed record SignatureElement(
    string Name,
    int SemanticIndex,
    string SystemValue,
    string ComponentType,
    int Register,
    string Mask,
    int ReadWriteMask,
    int MinPrecision);
