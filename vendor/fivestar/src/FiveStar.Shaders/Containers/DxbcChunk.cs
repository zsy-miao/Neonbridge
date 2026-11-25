// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Containers;

/// <summary>One chunk of a DXBC container: its four-character code and its payload.</summary>
public sealed record DxbcChunk(string FourCc, ReadOnlyMemory<byte> Data);
