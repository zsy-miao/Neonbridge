// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims;

/// <summary>A registered shim: the (module, symbol) it answers, its native entry point and the method behind it.</summary>
internal sealed record ShimEntry(string Module, string Symbol, nint Address, string DeclaringType, string MethodName);
