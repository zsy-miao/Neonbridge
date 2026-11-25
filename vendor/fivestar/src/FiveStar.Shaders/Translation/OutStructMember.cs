// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Translation;

/// <summary>
/// One member of a producer's <c>Out</c> struct: its declaration without interpolation modifiers
/// (<c>float2 p3_xy : TEXCOORD3;</c>), its name and its semantic split into name and index.
/// </summary>
internal sealed record OutStructMember(string Declaration, string Name, string Semantic, int SemanticIndex);
