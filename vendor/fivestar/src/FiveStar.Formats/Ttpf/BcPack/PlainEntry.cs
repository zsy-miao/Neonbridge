// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Ttpf.BcPack;

/// <summary>
/// A decoded BC1, BC3, BC4 or BC5 block as the block layer predicts from it: endpoint fields and per-pixel indices as
/// small unsigned values (indices along the endpoint line, not BC codes; BC4 endpoints 128 below their stored byte).
/// BC3 keeps R0 G0 B0 R1 G1 B1 then alpha e0 e1, alpha indices then colour indices; BC5 channel 0 then channel 1.
/// </summary>
internal unsafe struct PlainEntry
{
    public const int EndpointCount = 8;

    public const int IndexCount = 32;

    public fixed byte Endpoints[EndpointCount];

    public fixed byte Indices[IndexCount];
}
