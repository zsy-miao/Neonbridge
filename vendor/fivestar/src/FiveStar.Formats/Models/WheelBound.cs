// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Models;

/// <summary>The collision cylinder of one wheel of a vehicle fragment, in metres: tyre radius and width (along x).</summary>
public sealed record WheelBound(float Radius, float Width);
