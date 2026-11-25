// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Translation;

/// <summary>The Out struct of a translated mesh shader as a packed vertex record: its text, members and stride in bytes.</summary>
internal sealed record OutLayout(string StructText, IReadOnlyList<OutMember> Members, int Stride);
