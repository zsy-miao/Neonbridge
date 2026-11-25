// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Dxil.Container;

/// <summary>
/// A geometry shader's state: input primitive (DXIL InputPrimitive), output topology (DXIL PrimitiveTopology), the
/// most vertices it emits, and the streams it writes (bit mask).
/// </summary>
public sealed record GeometryState(int InputPrimitive, int OutputTopology, int MaxVertexCount, int StreamMask);
