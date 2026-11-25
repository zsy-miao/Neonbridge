// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Diagnostics;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;
using FiveStar.Loader.Shims.D3D12X.Binding;
using FiveStar.Loader.Shims.D3D12X.Pipelines;

namespace FiveStar.Loader.Shims.D3D12X;

/// <summary>
/// Device slot 14 and the handles of the game's two shader-visible heaps. Their descriptors live in title memory, as
/// on the console (<see cref="TitleMemoryHooks"/>): slot 9 returns that memory and slot 10 the same address, so a CPU
/// or GPU handle is the address of a descriptor's record (<see cref="ShaderVisibleHeap"/>). The view heap's game
/// indices reach the desktop heap through a <see cref="DescriptorRemap"/>; sampler indices are the desktop ones. Other
/// heaps use the desktop handles directly.
/// </summary>
internal sealed unsafe class XboxDescriptorHeaps
{
    /// <summary>ID3D12Device::CreateDescriptorHeap and ::GetDescriptorHandleIncrementSize (d3d12.h).</summary>
    private const int DesktopCreateSlot = 14;
    private const int DesktopIncrementSlot = 15;

    /// <summary>D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE (d3d12.h).</summary>
    private const int ShaderVisible = 1;

    /// <summary>D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV and _SAMPLER (d3d12.h).</summary>
    private const int ViewHeapType = 0;
    private const int SamplerHeapType = 1;

    /// <summary>
    /// XMemAlloc attributes of a descriptor range: allocator id 128 (bits 16..23) and object type 4 (bits 0..13), the
    /// pair the game's hook at 0x145eb2730 answers.
    /// </summary>
    private const ulong DescriptorAttributes = 0x00800004;

    /// <summary>Desktop pages (of <see cref="DescriptorRemap.PageSize"/>) behind the game's CPU-only view heaps: 16 pages, 65,536 writes before reuse.</summary>
    private const uint RingPages = 16;

    private readonly XboxObjects _objects;
    private readonly TitleMemoryHooks _hooks;
    private readonly DescriptorWindow _window;
    private readonly GameMemoryViews _memoryViews;
    private readonly LoaderLog _log;
    private readonly OnceLog _warnings;

    /// <summary>Title memory the shim allocated itself (no game image to ask), kept alive with the heaps.</summary>
    private readonly List<byte[]> _ownMemory = [];
    private ShaderVisibleHeap? _views;
    private ShaderVisibleHeap? _samplers;
    private ShaderVisibleHeap[] _cpuViews = [];
    private DescriptorRemap? _remap;
    private DescriptorRing? _ring;
    private HeapRecords _viewRecords;
    private nint _viewDesktopStart;
    private nint _sink;
    private long _tableBinds;
    private long _tableTicks;
    private long _reachMirrors;

    public XboxDescriptorHeaps(XboxObjects objects, TitleMemoryHooks hooks, DescriptorWindow window, GameMemoryViews memoryViews, LoaderLog log, OnceLog warnings)
    {
        _objects = objects;
        _hooks = hooks;
        _window = window;
        _memoryViews = memoryViews;
        _log = log;
        _warnings = warnings;
    }

    /// <summary>The game's shader-visible view heap; null before it exists.</summary>
    public ShaderVisibleHeap? Views => _views;

    /// <summary>The game's shader-visible sampler heap; null before it exists.</summary>
    public ShaderVisibleHeap? Samplers => _samplers;

    /// <summary>The desktop CPU handle a new descriptor at a game handle is written to (records updated); other handles pass through.</summary>
    [HotPath]
    public nint Destination(nint handle)
    {
        return Destination(handle, BufferRecord.None);
    }

    /// <summary>As <see cref="Destination(nint)"/> for a buffer view, whose record keeps its V# fields (<see cref="BufferRecords"/>).</summary>
    [HotPath]
    public nint Destination(nint handle, BufferRecord buffer)
    {
        var heap = Find(handle, out uint index);

        if (heap is null)
        {
            // A desktop heap's descriptor: its buffer fields are kept by handle for the copies into the game's heaps.
            DesktopRecords.Note(handle, buffer);

            return handle;
        }

        nint desktop = heap.Destination(index, 1, buffer);

        if (desktop == 0)
        {
            return Sink();
        }

        return desktop;
    }

    /// <summary>The desktop CPU handle of the descriptor a game handle names; other handles pass through.</summary>
    [HotPath]
    public nint Source(nint handle)
    {
        var heap = Find(handle, out uint index);

        if (heap is null)
        {
            return handle;
        }

        return heap.Source(index);
    }

    /// <summary>The shader-visible heap holding a handle, with the descriptor's index; null for other handles.</summary>
    [HotPath]
    public ShaderVisibleHeap? Find(nint handle, out uint index)
    {
        var views = _views;

        if (views is not null && views.TryIndex((ulong)handle, out index))
        {
            return views;
        }

        var samplers = _samplers;

        if (samplers is not null && samplers.TryIndex((ulong)handle, out index))
        {
            return samplers;
        }

        foreach (var heap in Volatile.Read(ref _cpuViews))
        {
            if (heap.TryIndex((ulong)handle, out index))
            {
                return heap;
            }
        }

        index = 0;

        return null;
    }

    /// <summary>The window pointer of a table at a GPU handle, its records mirrored; the null table for a handle in neither heap.</summary>
    [HotPath]
    public uint TableAddress(ulong handle, uint extent)
    {
        var heap = Find((nint)handle, out uint index);

        if (heap is null || !heap.Mirrored)
        {
            _warnings.Warn("table-handle", "[d3d12_xs] SetRootDescriptorTable: a GPU handle outside the shader-visible heaps reads null descriptors");

            return DescriptorWindow.NullTableBase;
        }

        long started = Stopwatch.GetTimestamp();
        uint table = heap.Table(index, extent);
        Interlocked.Add(ref _tableTicks, Stopwatch.GetTimestamp() - started);
        Interlocked.Increment(ref _tableBinds);

        return table;
    }

    /// <summary>
    /// Mirrors <paramref name="records"/> view records from the table at window offset <paramref name="windowOffset"/>
    /// (a user SGPR's table pointer, <see cref="Binding.TableReaches"/>); ignored outside the view records.
    /// </summary>
    [HotPath]
    public void MirrorTable(uint windowOffset, uint records)
    {
        var views = _views;

        if (views is null || windowOffset >= DescriptorWindow.NullTableBase || windowOffset % DescriptorWindow.ViewRecordBytes != 0)
        {
            return;
        }

        uint index = (windowOffset - DescriptorWindow.ViewBase) / DescriptorWindow.ViewRecordBytes;

        if (index < views.Count)
        {
            views.Table(index, records);
            Interlocked.Increment(ref _reachMirrors);
        }
    }

    /// <summary>Table binds so far and the time their mirroring took (<see cref="ShaderVisibleHeap.Table"/>).</summary>
    public string TableSummary => $"{Volatile.Read(ref _tableBinds)} table binds mirrored in {Stopwatch.GetElapsedTime(0, Volatile.Read(ref _tableTicks)).TotalMilliseconds:F0} ms, {Volatile.Read(ref _reachMirrors)} tables mirrored further at draws for programs' reach";

    /// <summary>The desktop sink for views no desktop descriptor is left for (warned once).</summary>
    [HotPath]
    public nint Sink()
    {
        _warnings.Warn("descriptor-limit", "[d3d12_xs] the game's descriptors touch more pages than the desktop heap holds: views written there are dropped");

        return _sink;
    }

    public int Create(nint device, DescriptorHeapDescription* description, Guid* iid, nint* heap)
    {
        var desktop = *description;
        bool visible = (desktop.Flags & ShaderVisible) != 0;

        if (visible && desktop.Type == ViewHeapType)
        {
            // Whole remap pages after the reserved descriptors, as far as the desktop allows.
            ulong pages = ((ulong)desktop.Count + DescriptorRemap.PageSize - 1) / DescriptorRemap.PageSize;
            desktop.Count = (uint)Math.Min(DescriptorWindow.ReservedViews + (pages * DescriptorRemap.PageSize), DescriptorWindow.MaxViews);
        }
        else if (visible && desktop.Type == SamplerHeapType)
        {
            // Room for the static samplers of the programs at STATIC_SAMPLER_BASE.
            desktop.Count = Math.Max(desktop.Count, DescriptorWindow.MaxSamplers);
        }

        int hr = CreateDesktop(device, &desktop, iid, heap);

        _log.Detail($"  [d3d12_xs] CreateDescriptorHeap(type {description->Type}, {description->Count} descriptors, flags 0x{description->Flags:x}, node 0x{description->NodeMask:x}) -> {HResult.Format(hr)}");

        if (!HResult.Failed(hr) && heap != null && !visible && desktop.Type == ViewHeapType && _views is not null)
        {
            *heap = _objects.Wrap(XboxInterfaceKind.DescriptorHeap, *heap);
            PlaceCpuOnly((XboxDescriptorHeapObject*)*heap, description->Count);

            return hr;
        }

        if (HResult.Failed(hr) || heap == null || !visible)
        {
            return _objects.WrapResult(hr, XboxInterfaceKind.DescriptorHeap, heap);
        }

        nint inner = *heap;
        *heap = _objects.Wrap(XboxInterfaceKind.DescriptorHeap, inner);
        Place(device, (XboxDescriptorHeapObject*)*heap, description, desktop.Count);

        return hr;
    }

    /// <summary>Puts the heap's descriptors in title memory and sets it up for binding.</summary>
    private void Place(nint device, XboxDescriptorHeapObject* heap, DescriptorHeapDescription* description, uint desktopCount)
    {
        uint increment = ((delegate* unmanaged<nint, int, uint>)ComObject.Slot(device, DesktopIncrementSlot))(device, description->Type);
        nint inner = heap->Header.Inner;
        bool views = description->Type == ViewHeapType && _views is null;
        bool samplers = description->Type == SamplerHeapType && _samplers is null;

        if (!views && !samplers)
        {
            return;
        }

        nint memory = TitleMemory((nuint)description->Count * increment);
        heap->GameCpuStart = memory;
        heap->GameGpuStart = memory;
        nint desktopStart = DesktopCalls.HeapCpuStart(inner);
        _log.Detail($"  [d3d12_xs]   CPU and GPU descriptors at title memory 0x{memory:x} (desktop 0x{desktopStart:x})");

        if (views)
        {
            _window.AttachViews(device, inner, description->Count, increment);
            var remap = new DescriptorRemap(DescriptorWindow.ReservedViews, desktopCount, description->Count);
            _sink = desktopStart + (nint)((ulong)DescriptorWindow.SinkSlot * increment);

            _remap = remap;
            _viewRecords = DescriptorWindow.ViewRecords(desktopCount);
            _viewDesktopStart = desktopStart;
            _views = new ShaderVisibleHeap(memory, description->Count, increment, desktopStart, _viewRecords, remap, _window, null);
            _memoryViews.Attach(device, desktopStart, increment, remap);
            _log.Detail($"  [d3d12_xs]   game views on {remap.Capacity} desktop pages of {DescriptorRemap.PageSize} descriptors, mapped on first write");
        }
        else if (samplers)
        {
            _window.AttachSamplers(device, inner, desktopCount, increment);
            _samplers = new ShaderVisibleHeap(memory, description->Count, increment, desktopStart, DescriptorWindow.SamplerRecords(desktopCount), null, _window, null);
        }
    }

    /// <summary>
    /// A CPU-only view heap in title memory: the game copies its descriptors' bytes into shader-visible tables itself,
    /// so its records name descriptors of the desktop shader-visible heap, taken from the ring on every write.
    /// </summary>
    private void PlaceCpuOnly(XboxDescriptorHeapObject* heap, uint count)
    {
        var views = _views!;
        _ring ??= Ring();

        if (_ring is null)
        {
            _warnings.Warn("cpu-heap-ring", "[d3d12_xs] no desktop pages left for CPU-only view heaps: their descriptors stay desktop ones");

            return;
        }

        nint memory = TitleMemory((nuint)count * views.Increment);
        new Span<byte>((void*)memory, (int)(count * views.Increment)).Clear();
        heap->GameCpuStart = memory;
        var placed = new ShaderVisibleHeap(memory, count, views.Increment, _viewDesktopStart, _viewRecords, null, _window, _ring);
        Volatile.Write(ref _cpuViews, [.. _cpuViews, placed]);
        _log.Detail($"  [d3d12_xs]   CPU-only view heap of {count} at title memory 0x{memory:x}, descriptors from the ring");
    }

    private DescriptorRing? Ring()
    {
        uint first = _remap!.Reserve(RingPages);

        if (first == DescriptorRemap.Unmapped)
        {
            return null;
        }

        return new DescriptorRing(first, RingPages * DescriptorRemap.PageSize);
    }

    /// <summary>The title's memory for a heap's descriptors; without a game image (tests), pinned memory of the shim's own.</summary>
    private nint TitleMemory(nuint bytes)
    {
        nint memory = _hooks.Allocate(bytes, DescriptorAttributes);

        if (memory != 0)
        {
            return memory;
        }

        byte[] own = GC.AllocateArray<byte>((int)bytes, pinned: true);
        _ownMemory.Add(own);

        fixed (byte* data = own)
        {
            return (nint)data;
        }
    }

    private static int CreateDesktop(nint device, DescriptorHeapDescription* description, Guid* iid, nint* heap)
    {
        var create = (delegate* unmanaged<nint, DescriptorHeapDescription*, Guid*, nint*, int>)ComObject.Slot(device, DesktopCreateSlot);

        return create(device, description, iid, heap);
    }
}
