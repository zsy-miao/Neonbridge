// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Models;

/// <summary>The stream-table slots whose meaning is known (see docs/models.md).</summary>
public static class VertexSlot
{
    public const int Position = 0;
    public const int Normal = 2;
    public const int TexCoord0 = 3;
    public const int TexCoord1 = 4;
    public const int Weights = 6;
    public const int SecondWeights = 7;
    public const int Joints = 8;
    public const int SecondJoints = 9;
    public const int Tangent = 11;
    public const int TexCoord2 = 14;
    public const int TexCoord3 = 15;
}
