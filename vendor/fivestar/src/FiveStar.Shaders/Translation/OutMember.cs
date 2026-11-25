// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Translation;

/// <summary>One Out struct member in the vertex record: name, scalar type (float, uint, int), width and byte offset.</summary>
internal sealed record OutMember(string Name, string Type, int Width, int Offset);
