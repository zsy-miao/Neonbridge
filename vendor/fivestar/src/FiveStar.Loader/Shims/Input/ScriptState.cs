// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.Input;

/// <summary>The script a <see cref="ScriptedDesktopInput"/> follows and which of its presses were logged (1).</summary>
internal sealed record ScriptState(InputScript Script, int[] Announced);
