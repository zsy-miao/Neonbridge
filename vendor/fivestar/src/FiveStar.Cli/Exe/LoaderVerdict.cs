// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Cli.Exe;

/// <summary>The loader's classification of one import: its class and the reason.</summary>
/// <param name="Kind">The resolution class the loader reported (<c>Shim</c>, <c>XboxStub</c>, <c>Package</c>, <c>Host</c>, <c>Missing</c>).</param>
/// <param name="Detail">The shim behind it or why nothing answers.</param>
public sealed record LoaderVerdict(string Kind, string Detail);
