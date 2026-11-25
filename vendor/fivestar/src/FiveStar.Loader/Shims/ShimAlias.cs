// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Text.RegularExpressions;

namespace FiveStar.Loader.Shims;

/// <summary>Module names matching <see cref="Pattern"/> are answered by <see cref="Target"/> for symbols with <see cref="Prefix"/>.</summary>
internal sealed record ShimAlias(Regex Pattern, string Prefix, string Target);
