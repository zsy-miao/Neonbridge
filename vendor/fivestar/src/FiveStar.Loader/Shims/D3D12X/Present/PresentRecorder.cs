// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.D3D12X.Present;

/// <summary>
/// The command list that puts a frame into a back buffer: one allocator per back buffer, retired by a fence, and a
/// render-target view per buffer for the test pattern. All desktop objects, called through d3d12.h slots.
/// </summary>
internal sealed unsafe class PresentRecorder : IDisposable
{
    /// <summary>ID3D12Device slots.</summary>
    private const int CreateCommandAllocatorSlot = 9;
    private const int CreateCommandListSlot = 12;
    private const int CreateDescriptorHeapSlot = 14;
    private const int GetDescriptorHandleIncrementSizeSlot = 15;
    private const int CreateRenderTargetViewSlot = 20;
    private const int CreateFenceSlot = 36;

    /// <summary>ID3D12CommandAllocator::Reset; ID3D12GraphicsCommandList Close, Reset, CopyTextureRegion, ResourceBarrier.</summary>
    private const int AllocatorResetSlot = 8;
    private const int CloseSlot = 9;
    private const int ListResetSlot = 10;
    private const int CopyTextureRegionSlot = 16;
    private const int ResourceBarrierSlot = 26;

    /// <summary>ID3D12CommandQueue ExecuteCommandLists, Signal; ID3D12Fence GetCompletedValue, SetEventOnCompletion.</summary>
    private const int ExecuteCommandListsSlot = 10;
    private const int SignalSlot = 14;
    private const int GetCompletedValueSlot = 8;
    private const int SetEventOnCompletionSlot = 9;

    /// <summary>ID3D12DescriptorHeap::GetCPUDescriptorHandleForHeapStart.</summary>
    private const int HeapCpuStartSlot = 9;

    /// <summary>D3D12_DESCRIPTOR_HEAP_TYPE_RTV; D3D12_COMMAND_LIST_TYPE_DIRECT.</summary>
    private const int RenderTargetHeap = 2;
    private const int DirectList = 0;

    private static readonly Guid AllocatorId = new("6102dee4-af59-4b09-b999-b44d73f09b24");
    private static readonly Guid HeapId = new("8efb471d-616c-4f49-90f7-127bb763fa51");
    private static readonly Guid FenceId = new("0a753dcf-c4d8-4b91-adf6-be5a60d95a76");

    private readonly nint _device;
    private readonly nint[] _allocators = new nint[DxgiSwapChain.BufferCount];
    private readonly ulong[] _retire = new ulong[DxgiSwapChain.BufferCount];
    private nint _list;
    private nint _fence;
    private nint _heap;
    private nint _event;
    private nint _heapStart;
    private uint _viewSize;
    private ulong _signaled;

    public PresentRecorder(nint device)
    {
        _device = device;
    }

    /// <summary>The list being recorded, between <see cref="Begin"/> and <see cref="Submit"/>.</summary>
    public nint List => _list;

    /// <summary>Allocators, the list (closed), the fence, its event and the view heap.</summary>
    public int Create()
    {
        int hr = CreateAllocators();

        if (HResult.Failed(hr))
        {
            return hr;
        }

        Guid listId = D3D12XLayout.GraphicsCommandListIid;
        nint list = 0;
        var createList = (delegate* unmanaged<nint, uint, int, nint, nint, Guid*, nint*, int>)ComObject.Slot(_device, CreateCommandListSlot);
        hr = createList(_device, 0, DirectList, _allocators[0], 0, &listId, &list);

        if (HResult.Failed(hr))
        {
            return hr;
        }

        _list = list;
        _ = ((delegate* unmanaged<nint, int>)ComObject.Slot(_list, CloseSlot))(_list);

        return CreateFenceAndHeap();
    }

    /// <summary>A render-target view per back buffer, after the swap chain made or resized them.</summary>
    public void SetTargets(ReadOnlySpan<nint> buffers)
    {
        var create = (delegate* unmanaged<nint, nint, void*, nint, void>)ComObject.Slot(_device, CreateRenderTargetViewSlot);

        for (int i = 0; i < buffers.Length; i++)
        {
            create(_device, buffers[i], null, View(i));
        }
    }

    /// <summary>The render-target view of back buffer <paramref name="index"/>.</summary>
    [HotPath]
    public nint View(int index)
    {
        return _heapStart + (nint)(index * _viewSize);
    }

    /// <summary>Waits until back buffer <paramref name="index"/>'s last frame is done, then opens the list on its allocator.</summary>
    [HotPath]
    public void Begin(uint index)
    {
        WaitFor(_retire[index]);
        nint allocator = _allocators[index];
        _ = ((delegate* unmanaged<nint, int>)ComObject.Slot(allocator, AllocatorResetSlot))(allocator);
        _ = ((delegate* unmanaged<nint, nint, nint, int>)ComObject.Slot(_list, ListResetSlot))(_list, allocator, 0);
    }

    [HotPath]
    public void Transition(nint resource, int before, int after)
    {
        var barrier = TransitionBarrier.Of(resource, before, after);
        ((delegate* unmanaged<nint, uint, TransitionBarrier*, void>)ComObject.Slot(_list, ResourceBarrierSlot))(_list, 1, &barrier);
    }

    /// <summary>Copies <paramref name="box"/> of subresource 0 of <paramref name="source"/> to (0, 0) of <paramref name="destination"/>.</summary>
    [HotPath]
    public void Copy(nint destination, nint source, CopyBox* box)
    {
        var to = new TextureCopyLocation { Resource = destination };
        var from = new TextureCopyLocation { Resource = source };
        var copy = (delegate* unmanaged<nint, TextureCopyLocation*, uint, uint, uint, TextureCopyLocation*, CopyBox*, void>)ComObject.Slot(_list, CopyTextureRegionSlot);
        copy(_list, &to, 0, 0, 0, &from, box);
    }

    /// <summary>Closes and runs the list on <paramref name="queue"/>, and marks back buffer <paramref name="index"/> busy.</summary>
    [HotPath]
    public void Submit(nint queue, uint index)
    {
        _ = ((delegate* unmanaged<nint, int>)ComObject.Slot(_list, CloseSlot))(_list);
        nint list = _list;
        ((delegate* unmanaged<nint, uint, nint*, void>)ComObject.Slot(queue, ExecuteCommandListsSlot))(queue, 1, &list);
        _retire[index] = Signal(queue);
    }

    /// <summary>Waits until the GPU has finished everything submitted on <paramref name="queue"/> so far.</summary>
    public void WaitIdle(nint queue)
    {
        WaitFor(Signal(queue));
    }

    public void Dispose()
    {
        foreach (nint allocator in _allocators)
        {
            ComObject.Release(allocator);
        }

        ComObject.Release(_list);
        ComObject.Release(_fence);
        ComObject.Release(_heap);

        if (_event != 0)
        {
            _ = PresentNative.CloseHandle(_event);
        }
    }

    [HotPath]
    private ulong Signal(nint queue)
    {
        _signaled++;
        _ = ((delegate* unmanaged<nint, nint, ulong, int>)ComObject.Slot(queue, SignalSlot))(queue, _fence, _signaled);

        return _signaled;
    }

    [HotPath]
    private void WaitFor(ulong value)
    {
        ulong done = ((delegate* unmanaged<nint, ulong>)ComObject.Slot(_fence, GetCompletedValueSlot))(_fence);

        if (value == 0 || done >= value)
        {
            return;
        }

        _ = ((delegate* unmanaged<nint, ulong, nint, int>)ComObject.Slot(_fence, SetEventOnCompletionSlot))(_fence, value, _event);
        _ = PresentNative.WaitForSingleObject(_event, PresentNative.Infinite);
    }

    private int CreateAllocators()
    {
        Guid allocatorId = AllocatorId;
        var create = (delegate* unmanaged<nint, int, Guid*, nint*, int>)ComObject.Slot(_device, CreateCommandAllocatorSlot);

        for (int i = 0; i < _allocators.Length; i++)
        {
            nint allocator = 0;
            int hr = create(_device, DirectList, &allocatorId, &allocator);

            if (HResult.Failed(hr))
            {
                return hr;
            }

            _allocators[i] = allocator;
        }

        return HResult.Ok;
    }

    private int CreateFenceAndHeap()
    {
        Guid fenceId = FenceId;
        nint fence = 0;
        int hr = ((delegate* unmanaged<nint, ulong, int, Guid*, nint*, int>)ComObject.Slot(_device, CreateFenceSlot))(_device, 0, 0, &fenceId, &fence);

        if (HResult.Failed(hr))
        {
            return hr;
        }

        _fence = fence;
        _event = PresentNative.CreateEventW(0, 0, 0, null);
        var description = new DescriptorHeapDescription { Type = RenderTargetHeap, Count = DxgiSwapChain.BufferCount };
        Guid heapId = HeapId;
        nint heap = 0;
        hr = ((delegate* unmanaged<nint, DescriptorHeapDescription*, Guid*, nint*, int>)ComObject.Slot(_device, CreateDescriptorHeapSlot))(_device, &description, &heapId, &heap);

        if (HResult.Failed(hr))
        {
            return hr;
        }

        _heap = heap;
        nint start = 0;
        _ = ((delegate* unmanaged<nint, nint*, nint*>)ComObject.Slot(_heap, HeapCpuStartSlot))(_heap, &start);
        _heapStart = start;
        _viewSize = ((delegate* unmanaged<nint, int, uint>)ComObject.Slot(_device, GetDescriptorHandleIncrementSizeSlot))(_device, RenderTargetHeap);

        return HResult.Ok;
    }
}
