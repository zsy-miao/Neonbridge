// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims;

/// <summary>One (module, name) a [Shim] method answers, as the generated registration passes it.</summary>
internal readonly record struct ShimBinding(string Module, string Name);
