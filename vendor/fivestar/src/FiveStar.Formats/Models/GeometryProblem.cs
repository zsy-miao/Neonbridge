// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Models;

/// <summary>A geometry that did not decode: its level of detail, virtual offset and the reason.</summary>
public sealed record GeometryProblem(int Lod, int Offset, string Reason);
