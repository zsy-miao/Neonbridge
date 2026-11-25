// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Translation.Structure;

/// <summary>
/// A natural loop: its header (which dominates the body), the body nodes (header included), the header of the
/// innermost enclosing loop, and the nodes outside the body that the body branches to, in reverse postorder.
/// </summary>
internal sealed record NaturalLoop(int Header, IReadOnlySet<int> Body, int? Parent, IReadOnlyList<int> Exits);
