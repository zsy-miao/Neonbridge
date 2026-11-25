// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Ttpf.BcPack;

/// <summary>
/// A decoded BC6H block as the block layer predicts from it: four endpoints of R G B (endpoint 0, then 1 to 3), each
/// left-aligned in 16 bits with its top bit flipped, and the mode (1-14) with 16 signed indices centred on 0.
/// </summary>
internal unsafe struct Bc6HEntry
{
    public const int EndpointCount = 4;

    public const int Channels = 3;

    public const int EndpointValues = EndpointCount * Channels;

    public const int IndexCount = 16;

    /// <summary>Endpoint k channel c at <c>k * 3 + c</c>.</summary>
    public fixed ushort Endpoints[EndpointValues];

    public byte Mode;

    public fixed short Indices[IndexCount];
}
