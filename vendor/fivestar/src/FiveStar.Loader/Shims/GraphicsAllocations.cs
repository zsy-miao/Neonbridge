// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Collections.Concurrent;

namespace FiveStar.Loader.Shims;

/// <summary>
/// The GPU access of each plain XMemVirtualAlloc allocation, by the base it returned. On the console the GPU reaches
/// only memory allocated as graphics memory (MEM_GRAPHICS or a PAGE_GRAPHICS_* protection); the D3D12 shim gives
/// game memory outside that a read-only GPU view (<see cref="D3D12X.XboxGameMemoryBuffers"/>).
/// </summary>
internal sealed class GraphicsAllocations
{
    /// <summary>winnt.h (console) MEM_GRAPHICS.</summary>
    public const uint MemGraphics = 0x10000000;

    /// <summary>winnt.h (console) PAGE_GRAPHICS_READONLY, _READWRITE, _EXECUTE, _EXECUTE_READ, _EXECUTE_READWRITE.</summary>
    private const uint GraphicsReadOnly = 0x1000;
    private const uint GraphicsReadWrite = 0x2000;
    private const uint GraphicsExecute = 0x4000;
    private const uint GraphicsExecuteRead = 0x8000;
    private const uint GraphicsExecuteReadWrite = 0x10000;

    private readonly ConcurrentDictionary<nint, (GraphicsAccess Access, uint Type, uint Protect)> _bases = new();
    private readonly ConcurrentDictionary<nint, ulong> _sizes = new();

    /// <summary>The GPU access an allocation type and protection give.</summary>
    public static GraphicsAccess Classify(uint allocationType, uint protect)
    {
        if ((protect & (GraphicsReadWrite | GraphicsExecuteReadWrite)) != 0)
        {
            return GraphicsAccess.ReadWrite;
        }

        if ((protect & (GraphicsReadOnly | GraphicsExecute | GraphicsExecuteRead)) != 0)
        {
            return GraphicsAccess.Read;
        }

        if ((allocationType & MemGraphics) != 0)
        {
            return GraphicsAccess.ReadWrite;
        }

        return GraphicsAccess.None;
    }

    public void Add(nint allocationBase, ulong size, uint allocationType, uint protect)
    {
        if (allocationBase != 0)
        {
            _bases[allocationBase] = (Classify(allocationType, protect), allocationType, protect);
            _sizes[allocationBase] = size;
        }
    }

    /// <summary>The allocations (base, bytes) whose protection has all of <paramref name="protectBits"/>.</summary>
    public IReadOnlyList<(nint Base, ulong Bytes)> Matching(uint protectBits)
    {
        return _bases.Where(entry => (entry.Value.Protect & protectBits) == protectBits)
            .Select(entry => (entry.Key, _sizes.GetValueOrDefault(entry.Key)))
            .ToList();
    }

    /// <summary>The access of the allocation at <paramref name="allocationBase"/>; false for memory not allocated here.</summary>
    public bool TryGet(nint allocationBase, out GraphicsAccess access, out uint allocationType, out uint protect)
    {
        bool found = _bases.TryGetValue(allocationBase, out var entry);
        (access, allocationType, protect) = entry;

        return found;
    }
}
