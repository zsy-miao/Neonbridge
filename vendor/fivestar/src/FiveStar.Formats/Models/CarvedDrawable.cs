// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Models;

/// <summary>A drawable read from an archive: the offset of its virtual segment and whether a physical segment followed.</summary>
public sealed record CarvedDrawable(long Offset, bool HasPhysical, Drawable Drawable);
