// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Pso;

/// <summary>One chunk of a PSO file: its four-letter tag, offset in the file and size including the 8-byte header.</summary>
public readonly record struct PsoChunk(string Tag, int Offset, int Size);
