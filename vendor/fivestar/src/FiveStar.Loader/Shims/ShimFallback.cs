// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims;

/// <summary>
/// The answer for a symbol without a shim in an Xbox-only module: the value its stub returns, and whether the function
/// is a known, deliberately unimplemented one (as opposed to an unknown symbol).
/// </summary>
internal sealed record ShimFallback(long ReturnValue, bool NotImplemented);
