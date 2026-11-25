// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X.Resources;

/// <summary>Subresource 0 of a desktop texture read back to the CPU: tight rows of <paramref name="RowBytes"/> bytes.</summary>
/// <param name="Width">Pixels per row.</param>
/// <param name="Height">Rows.</param>
/// <param name="Format">The texture's DXGI format.</param>
/// <param name="RowBytes">Bytes per row in <paramref name="Pixels"/>.</param>
/// <param name="Pixels">The texels as the GPU stores them.</param>
internal sealed record ReadbackImage(int Width, int Height, int Format, int RowBytes, byte[] Pixels);
