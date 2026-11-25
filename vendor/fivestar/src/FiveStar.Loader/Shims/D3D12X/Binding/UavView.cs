// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X.Binding;

/// <summary>A texture UAV: its DXGI format, D3D12_UAV_DIMENSION and the size of the viewed mip (slices for an array or a 3D texture).</summary>
internal readonly record struct UavView(int Format, int Dimension, uint Width, uint Height, uint Slices);
