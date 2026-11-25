// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Models;

/// <summary>
/// The skin influences of a geometry: <paramref name="Influences"/> (1, 4 or 8) joint indices and weights per vertex.
/// Weights are bytes that sum to 255 per vertex. On vehicles a joint index is a bone of the drawable's skeleton; on
/// peds it is an entry of the model's bone palette (docs/models.md).
/// </summary>
public sealed record VertexSkin(int Influences, ushort[] Joints, byte[] Weights);
