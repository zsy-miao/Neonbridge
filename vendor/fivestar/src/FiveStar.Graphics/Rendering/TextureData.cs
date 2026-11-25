// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using Vortice.DXGI;

namespace FiveStar.Graphics.Rendering;

/// <summary>
/// One mip of a 2D texture as row-major rows: <see cref="Rows"/> rows of <see cref="RowBytes"/> bytes each (a row is
/// a row of 4x4 blocks for block-compressed formats).
/// </summary>
public sealed record TextureData(Format Format, int Width, int Height, int RowBytes, int Rows, byte[] Data);
