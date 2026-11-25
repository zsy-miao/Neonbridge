// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Translation.Stages;

/// <summary>The private LDS layout of a translated geometry invocation: output records first, input vertices from InputBase.</summary>
internal sealed record GeometryLayout(int RecordDwords, int InputStride, int InputBase, int MaxVertices, int InputVertices);
