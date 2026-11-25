// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.Input;

/// <summary>
/// One cursor motion of an input script: from <paramref name="Start"/> seconds after its block began, for
/// <paramref name="Hold"/> seconds, the cursor moves <paramref name="PerSecondX"/>, <paramref name="PerSecondY"/> pixels
/// a second (the right stick of the keyboard-and-mouse gamepad).
/// </summary>
internal readonly record struct ScriptedMotion(int Block, double Start, double Hold, double PerSecondX, double PerSecondY);
