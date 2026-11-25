// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Hardware;

/// <summary>The SGPR that the hardware preloads with one thread-group id component (axis x, y or z).</summary>
public readonly record struct GroupIdSgpr(char Axis, int Sgpr);
