// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X;

/// <summary>The D3D12.X interfaces the shim wraps; each has one vtable (<see cref="XboxInterfaces"/>).</summary>
internal enum XboxInterfaceKind
{
    Device,
    CommandQueue,
    CommandAllocator,
    CommandList,
    Resource,
    Heap,
    Fence,
    DescriptorHeap,
    QueryHeap,
    CommandSignature,
    PipelineState,
}
