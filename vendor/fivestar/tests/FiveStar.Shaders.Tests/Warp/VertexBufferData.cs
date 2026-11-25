// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Tests.Warp;

/// <summary>One vertex buffer: its stride in bytes and contents.</summary>
public sealed record VertexBufferData(int Stride, uint[] Words);
