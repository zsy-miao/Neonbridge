// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.Input;

/// <summary>
/// A <c>capture</c> or <c>learn &lt;name&gt;</c> line of an input script, <paramref name="Start"/> seconds after its
/// block began; <paramref name="Learn"/> is null for a capture.
/// </summary>
internal readonly record struct ScriptedMark(int Block, double Start, string? Learn);
