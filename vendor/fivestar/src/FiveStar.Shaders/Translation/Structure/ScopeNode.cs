// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Translation.Structure;

/// <summary>A node of a <see cref="ScopeGraph"/>: its kind and the flow-graph node (block, loop header or exit target).</summary>
internal readonly record struct ScopeNode(ScopeNodeKind Kind, int Node);
