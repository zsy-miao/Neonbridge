// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.CompilerServices;

namespace FiveStar.Loader.Shims.D3D12X;

/// <summary>
/// The wrapped interfaces and their slot maps (docs/d3d12x-shim.md). Desktop slot numbers are the d3d12.h vtable
/// order. Slots 0 (QueryInterface), 2 (Release) and, on device children, 7 (GetDevice) are handlers bound by
/// <see cref="XboxObjects"/>; the creation slots of the device are handlers bound by <see cref="XboxDeviceFactory"/>.
/// </summary>
internal static class XboxInterfaces
{
    /// <summary>The vtable stride: every interface's vtable has room for this many slots.</summary>
    public const int MaxSlotCount = 160;

    /// <summary>
    /// Bytes of a command-list wrapper: the game reads and writes fields up to +0x564 (inline recording); the shim's
    /// per-list state follows from 0x800 (binding, submission, activity, rectangle lists, segments, replay state, waits).
    /// </summary>
    public const int CommandListObjectSize = 0x2000;

    /// <summary>Command-list slot 139, read as Reset(allocator, PSO, flags) (*inferred*: call at 0x142474cce).</summary>
    private const int ResetXSlot = 139;

    /// <summary>Slots of the interfaces whose highest known slot is low (queue 35, resource 14).</summary>
    private const int SmallSlotCount = 64;

    private static readonly int PlainObjectSize = Unsafe.SizeOf<XboxObject>();
    private static readonly int DeviceObjectSize = Unsafe.SizeOf<XboxDeviceObject>();
    private static readonly int ResourceObjectSize = Unsafe.SizeOf<XboxResourceObject>();
    private static readonly int DescriptorHeapObjectSize = Unsafe.SizeOf<XboxDescriptorHeapObject>();
    private static readonly int FenceObjectSize = Unsafe.SizeOf<XboxFenceObject>();
    private static readonly int QueueObjectSize = Unsafe.SizeOf<XboxQueueObject>();

    /// <summary>AddRef and ID3D12Object (GetPrivateData, SetPrivateData, SetPrivateDataInterface, SetName).</summary>
    private static readonly ForwardedSlot[] Common =
    [
        Same(1), Same(3), Same(4), new(5, 5, false, ArgumentRegisters.R8), Same(6),
    ];

    /// <summary>ID3D12Device slots 1 to 43 in desktop order; struct returns 25, 26, 43; views take a resource.</summary>
    private static readonly ForwardedSlot[] DeviceSlots = Enumerable.Range(1, 43)
        .Where(slot => slot != 2)
        .Select(slot => Same(slot))
        .Concat([Swap(25), Swap(26), Swap(43)])
        .Concat([Unwrapped(18, ArgumentRegisters.Rdx), Unwrapped(19, ArgumentRegisters.Rdx | ArgumentRegisters.R8)])
        .Concat([Unwrapped(20, ArgumentRegisters.Rdx), Unwrapped(21, ArgumentRegisters.Rdx), Unwrapped(31, ArgumentRegisters.Rdx)])
        .Concat([Unwrapped(42, ArgumentRegisters.Rdx)])
        .ToArray();

    /// <summary>
    /// ID3D12CommandQueue two slots later than desktop (Signal 16, GetTimestampFrequency 18 seen at call sites).
    /// 10 and 11 (tile mappings) carry a heap on the stack and 12 (ExecuteCommandLists) an array: handlers or stubs.
    /// </summary>
    private static readonly ForwardedSlot[] QueueSlots =
    [
        new(13, 11, false, ArgumentRegisters.None), new(14, 12, false, ArgumentRegisters.None),
        new(15, 13, false, ArgumentRegisters.None), new(16, 14, false, ArgumentRegisters.Rdx),
        new(17, 15, false, ArgumentRegisters.Rdx), new(18, 16, false, ArgumentRegisters.None),
        new(19, 17, false, ArgumentRegisters.None), new(20, 18, true, ArgumentRegisters.None),
    ];

    /// <summary>ID3D12CommandAllocator::Reset at desktop slot 8 (*inferred*: no X call site seen).</summary>
    private static readonly ForwardedSlot[] AllocatorSlots = [Same(8)];

    /// <summary>
    /// ID3D12GraphicsCommandList: 8 to 25 in desktop order (20, IASetPrimitiveTopology, is a handler); 33 to 40 and 50 to 52 four slots later; 70 is
    /// OMSetDepthBounds; 139 is Reset with a third argument (allocator, null, 0 when a pooled list is reused). Slots taking GPU addresses (41 to 48) wait for the address translation and stay stubs.
    /// </summary>
    private static readonly ForwardedSlot[] ListSlots =
    [
        Same(8), Same(9), Unwrapped(10, ArgumentRegisters.Rdx | ArgumentRegisters.R8), Unwrapped(11, ArgumentRegisters.Rdx),
        Same(12), Same(13), Same(14), Unwrapped(15, ArgumentRegisters.Rdx | ArgumentRegisters.R9),
        Unwrapped(17, ArgumentRegisters.Rdx | ArgumentRegisters.R8), Unwrapped(19, ArgumentRegisters.Rdx | ArgumentRegisters.R9),
        Same(21), Same(22), Same(23), Same(24), Unwrapped(25, ArgumentRegisters.Rdx),
        Moved(33, 29), Moved(34, 30), Moved(35, 31), Moved(36, 32), Moved(37, 33), Moved(38, 34), Moved(39, 35), Moved(40, 36),
        Moved(50, 46), Moved(51, 47), Moved(52, 48), Moved(70, 62),
        new(ResetXSlot, 10, false, ArgumentRegisters.Rdx | ArgumentRegisters.R8),
    ];

    /// <summary>
    /// ID3D12Resource: Map, Unmap, GetDesc, GetGPUVirtualAddress, Write/ReadFromSubresource, GetHeapProperties. GetDesc
    /// takes the resource in rcx and the result in rdx, the desktop order (call at 0x14501f80b).
    /// </summary>
    private static readonly ForwardedSlot[] ResourceSlots = [Same(8), Same(9), Same(10), Same(11), Same(12), Same(13), Same(14)];

    /// <summary>ID3D12Heap::GetDesc and ID3D12DescriptorHeap::GetDesc (struct returns).</summary>
    private static readonly ForwardedSlot[] DescriptionSlots = [Swap(8)];

    /// <summary>ID3D12Fence: GetCompletedValue, SetEventOnCompletion, Signal.</summary>
    private static readonly ForwardedSlot[] FenceSlots = [Same(8), Same(9), Same(10)];

    /// <summary>ID3D12PipelineState::GetCachedBlob.</summary>
    private static readonly ForwardedSlot[] PipelineSlots = [Same(8)];

    private static readonly XboxInterface[] All =
    [
        new(XboxInterfaceKind.Device, "ID3D12Device", MaxSlotCount, DeviceObjectSize, DeviceSlots),
        Child(XboxInterfaceKind.CommandQueue, "ID3D12CommandQueue", QueueObjectSize, QueueSlots),
        Child(XboxInterfaceKind.CommandAllocator, "ID3D12CommandAllocator", PlainObjectSize, AllocatorSlots),
        new(XboxInterfaceKind.CommandList, "ID3D12GraphicsCommandList", MaxSlotCount, CommandListObjectSize, [.. Common, .. ListSlots]),
        Child(XboxInterfaceKind.Resource, "ID3D12Resource", ResourceObjectSize, ResourceSlots),
        Child(XboxInterfaceKind.Heap, "ID3D12Heap", PlainObjectSize, DescriptionSlots),
        Child(XboxInterfaceKind.Fence, "ID3D12Fence", FenceObjectSize, FenceSlots),
        Child(XboxInterfaceKind.DescriptorHeap, "ID3D12DescriptorHeap", DescriptorHeapObjectSize, DescriptionSlots),
        Child(XboxInterfaceKind.QueryHeap, "ID3D12QueryHeap", PlainObjectSize, []),
        Child(XboxInterfaceKind.CommandSignature, "ID3D12CommandSignature", PlainObjectSize, []),
        Child(XboxInterfaceKind.PipelineState, "ID3D12PipelineState", PlainObjectSize, PipelineSlots),
    ];

    /// <summary>Number of wrapped interfaces (the vtable block holds this many vtables).</summary>
    public static int Count => All.Length;

    public static XboxInterface For(XboxInterfaceKind kind)
    {
        return All[(int)kind];
    }

    private static XboxInterface Child(XboxInterfaceKind kind, string name, int objectSize, ForwardedSlot[] slots)
    {
        return new XboxInterface(kind, name, SmallSlotCount, objectSize, [.. Common, .. slots]);
    }

    private static ForwardedSlot Same(int slot)
    {
        return new ForwardedSlot(slot, slot, false, ArgumentRegisters.None);
    }

    private static ForwardedSlot Swap(int slot)
    {
        return new ForwardedSlot(slot, slot, true, ArgumentRegisters.None);
    }

    private static ForwardedSlot Moved(int slot, int desktopSlot)
    {
        return new ForwardedSlot(slot, desktopSlot, false, ArgumentRegisters.None);
    }

    private static ForwardedSlot Unwrapped(int slot, ArgumentRegisters registers)
    {
        return new ForwardedSlot(slot, slot, false, registers);
    }
}
