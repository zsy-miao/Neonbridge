// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.CompilerServices;

namespace FiveStar.Loader.Shims.Input;

/// <summary>The pressed keys of one keyboard reading, inline.</summary>
[InlineArray(InputLimits.MaxKeys)]
internal struct KeyBuffer
{
    private KeyState _first;
}
