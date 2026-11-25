// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using Vortice.DXGI;

namespace FiveStar.Shaders.Tests.Warp;

/// <summary>One input-layout element (semantic ATTR&lt;n&gt; as the translated fetch inputs expect).</summary>
public sealed record VertexElement(string Semantic, int Index, Format Format, int Offset, int Slot, bool PerInstance);
