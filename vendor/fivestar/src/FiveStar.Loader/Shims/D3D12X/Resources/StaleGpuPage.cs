// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X.Resources;

/// <summary>A frame a desktop buffer still holds after the game mapped another one in its place.</summary>
/// <param name="GpuAddress">The console GPU address the buffer reaches the frame at.</param>
/// <param name="CpuAddress">Where the CPU had the frame mapped when it was replaced.</param>
/// <param name="BufferAddress">The console GPU address of the buffer's first byte.</param>
internal readonly record struct StaleGpuPage(ulong GpuAddress, nint CpuAddress, ulong BufferAddress);
