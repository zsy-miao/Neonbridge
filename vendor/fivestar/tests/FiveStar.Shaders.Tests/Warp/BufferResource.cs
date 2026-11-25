// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Tests.Warp;

/// <summary>
/// A raw buffer at a <c>ResourceDescriptorHeap</c> slot: a ByteAddressBuffer SRV, or a RWByteAddressBuffer UAV that is
/// read back after the run. Heap slot 0 is the GPU-memory window (GPU_MEM_HEAP), slot 1 its writable view.
/// </summary>
public sealed record BufferResource(int HeapIndex, uint[] Words, bool Writable);
