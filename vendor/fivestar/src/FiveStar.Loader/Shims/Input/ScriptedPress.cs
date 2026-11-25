// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.Input;

/// <summary>One key press of an input script: held from <paramref name="Start"/> seconds after its block began, for <paramref name="Hold"/> seconds.</summary>
internal readonly record struct ScriptedPress(int Block, double Start, double Hold, int Key, string Name);
