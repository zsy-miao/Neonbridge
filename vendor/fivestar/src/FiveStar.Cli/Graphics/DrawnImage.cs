// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Cli.Graphics;

/// <summary>An offscreen draw's RGBA8 pixels (rows top first) and the debug layer's errors, if it ran.</summary>
public sealed record DrawnImage(int Width, int Height, byte[] Pixels, IReadOnlyList<string> Errors);
