// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Models;

/// <summary>The stream format bytes the readers decode.</summary>
public static class VertexFormat
{
    /// <summary>8 bytes: a packed 24/24/16-bit position in slot 0, two floats elsewhere.</summary>
    public const byte Eight = 0x42;

    /// <summary>12 bytes: three floats (positions and normals of cables and water).</summary>
    public const byte Float3 = 0x63;

    /// <summary>4 bytes: octahedral normal, 10 + 10 bits (the top 12 bits are not decoded).</summary>
    public const byte PackedNormal = 0x21;

    /// <summary>4 bytes: two signed 16-bit values in units of 1/8192.</summary>
    public const byte Fixed16x2 = 0x28;

    /// <summary>4 bytes: four unsigned bytes (colours, skin weights).</summary>
    public const byte Bytes4 = 0x26;

    /// <summary>4 bytes: tangent x, y, z in 10 bits each and the bitangent sign in bits 30-31.</summary>
    public const byte Tangent = 0x27;

    /// <summary>4 bytes: four 8-bit joint indices.</summary>
    public const byte Joints4x8 = 0x25;

    /// <summary>8 bytes: four 16-bit joint indices.</summary>
    public const byte Joints4x16 = 0x4A;

    /// <summary>One joint per vertex with weight 1, a u8 or a u16: only the stream's size tells which.</summary>
    public const byte SingleJoint = 0x00;
}
