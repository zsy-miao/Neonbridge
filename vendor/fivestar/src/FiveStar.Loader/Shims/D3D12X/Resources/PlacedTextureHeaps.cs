// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Collections.Concurrent;
using System.Runtime.InteropServices;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;
using FiveStar.Loader.Shims.D3D12X.Binding;

namespace FiveStar.Loader.Shims.D3D12X.Resources;

/// <summary>
/// Read-only console textures placed in shared heaps (<c>--placed-textures</c>, docs/gpu-resources.md) instead of a
/// committed resource each: a cold boot made 22,000 committed textures by 275 s, and the driver's write-combined system
/// memory grew by about 0.55 MB per texture (17 GB). Heaps of <see cref="HeapBytes"/> (D3D12_HEAP_FLAG_ALLOW_ONLY_NON_RT_DS_TEXTURES,
/// which every heap tier takes) are carved first fit at the resource's alignment; a texture larger than a quarter heap
/// gets a heap of its own. A range goes back to its heap when the texture is destroyed: a small COM object set as the
/// resource's private data is released by the runtime then. The registry releases retired textures only after their
/// grace period, so a range is never reused while the GPU may still read the old texture; the new one is written whole
/// by its upload before any read. GPU-written and multisampled textures stay committed.
/// </summary>
internal sealed unsafe class PlacedTextureHeaps
{
    /// <summary>Bytes in a shared heap.</summary>
    public const ulong HeapBytes = 256UL << 20;

    // ID3D12Device GetResourceAllocationInfo, CreateHeap, CreatePlacedResource; ID3D12Object SetPrivateDataInterface (d3d12.h).
    private const int AllocationInfoSlot = 25;
    private const int CreateHeapSlot = 28;
    private const int CreatePlacedResourceSlot = 29;
    private const int SetPrivateDataInterfaceSlot = 5;

    /// <summary>D3D12_HEAP_TYPE_DEFAULT and D3D12_HEAP_FLAG_ALLOW_ONLY_NON_RT_DS_TEXTURES.</summary>
    private const int DefaultHeapType = 1;
    private const int NonTargetTexturesOnly = 0x44;
    private const ulong DefaultAlignment = 64UL << 10;
    private const int OwnHeapShare = 4;

    /// <summary>Empty shared heaps kept for new textures; any more go back.</summary>
    private const int KeptEmptyHeaps = 1;

    private const long DefaultEmptyMilliseconds = 60_000;
    private const double BytesPerGigabyte = 1UL << 30;

    private static readonly Guid ResourceIid = new("696442be-a72e-4059-bc79-5b5c98040fad");
    private static readonly Guid HeapIid = new("6b3b2502-6e51-45b3-90ee-9884265e8df3");
    private static readonly Guid UnknownIid = new("00000000-0000-0000-c000-000000000046");

    /// <summary>The private-data key the range's owner is set under.</summary>
    private static readonly Guid OwnerKey = new("4b1f5f2e-7a3c-4a55-9e0b-5f53a2a0d3c1");

    private static readonly ConcurrentDictionary<long, (PlacedTextureHeaps Heaps, Heap Heap, ulong Offset, ulong Size)> Owned = new();
    private static readonly OwnerMethods* OwnerVtable = CreateOwnerVtable();
    private static long _nextOwner;

    private readonly object _gate = new();
    private readonly List<Heap> _heaps = [];
    private long _placed;
    private long _freed;
    private long _returned;

    /// <summary>Whether read-only textures are placed (on by default; <c>--committed-textures</c> turns it off).</summary>
    public static bool Enabled { get; set; }

    /// <summary>Bytes of the shared heaps now.</summary>
    public long Bytes
    {
        get
        {
            lock (_gate)
            {
                ulong bytes = 0;

                foreach (var heap in _heaps)
                {
                    bytes += heap.Size;
                }

                return (long)bytes;
            }
        }
    }

    /// <summary>"placed textures: N live in H heaps (G GB, U GB used), R heaps returned", for the texture counters.</summary>
    public string Summary
    {
        get
        {
            lock (_gate)
            {
                ulong bytes = 0;
                ulong used = 0;

                foreach (var heap in _heaps)
                {
                    bytes += heap.Size;
                    used += heap.Used;
                }

                return $"placed textures: {Interlocked.Read(ref _placed) - Interlocked.Read(ref _freed)} live in {_heaps.Count} heaps ({bytes / BytesPerGigabyte:F1} GB, {used / BytesPerGigabyte:F1} GB used), {_returned} empty heaps returned";
            }
        }
    }

    /// <summary>A texture of <paramref name="description"/> placed in a shared heap; the HRESULT, or a failure when it must be committed.</summary>
    public int Create(nint device, ResourceDescription* description, int state, void* clear, out nint resource)
    {
        resource = 0;
        ResourceAllocationInfo info;
        var query = (delegate* unmanaged<nint, ResourceAllocationInfo*, uint, uint, ResourceDescription*, ResourceAllocationInfo*>)ComObject.Slot(device, AllocationInfoSlot);
        query(device, &info, 0, 1, description);

        if (info.Size == 0 || info.Size == ulong.MaxValue)
        {
            return HResult.InvalidArgument;
        }

        ulong alignment = Math.Max(info.Alignment, DefaultAlignment);

        if (!Take(device, info.Size, alignment, out var heap, out ulong offset))
        {
            return HResult.OutOfMemory;
        }

        Guid iid = ResourceIid;
        nint created = 0;
        var create = (delegate* unmanaged<nint, nint, ulong, ResourceDescription*, int, void*, Guid*, nint*, int>)ComObject.Slot(device, CreatePlacedResourceSlot);
        int hr = create(device, heap.Desktop, offset, description, state, clear, &iid, &created);

        if (HResult.Failed(hr) || created == 0)
        {
            Give(heap, offset, info.Size);

            return HResult.Fail;
        }

        Own(created, heap, offset, info.Size);
        Interlocked.Increment(ref _placed);
        resource = created;

        return hr;
    }

    /// <summary>A range of <paramref name="size"/> at <paramref name="alignment"/>: first fit in a shared heap, else a new heap.</summary>
    private bool Take(nint device, ulong size, ulong alignment, out Heap heap, out ulong offset)
    {
        bool own = size > HeapBytes / OwnHeapShare;

        lock (_gate)
        {
            if (!own)
            {
                foreach (var shared in _heaps)
                {
                    if (!shared.Own && shared.TryTake(size, alignment, out offset))
                    {
                        heap = shared;

                        return true;
                    }
                }
            }

            ulong bytes = HeapBytes;

            if (own)
            {
                bytes = (size + alignment - 1) & ~(alignment - 1);
            }

            nint desktop = CreateHeap(device, bytes, alignment);

            if (desktop == 0)
            {
                heap = null!;
                offset = 0;

                return false;
            }

            heap = new Heap(desktop, bytes, own);
            _heaps.Add(heap);

            return heap.TryTake(size, alignment, out offset);
        }
    }

    /// <summary>Gives a range back; a heap of its own is released with its texture.</summary>
    private void Give(Heap heap, ulong offset, ulong size)
    {
        lock (_gate)
        {
            if (heap.Own)
            {
                _heaps.Remove(heap);
                ComObject.Release(heap.Desktop);

                return;
            }

            heap.Give(offset, size);
            ReturnEmpty(Environment.TickCount64);
        }
    }

    /// <summary>
    /// Shared heaps empty for <see cref="EmptyMilliseconds"/> go back to the driver, with their video memory (charged
    /// to commit: boot w36 held 58 heaps, 14.5 GB, when commit reached its limit), past one kept for the next textures.
    /// A released texture's range stays mapped while its heap lives, so a stale read the game still makes hits memory
    /// (issue #23); a heap goes only after it has been empty that long, when the game has rewritten its descriptors.
    /// Call under the gate.
    /// </summary>
    private void ReturnEmpty(long now)
    {
        var empty = _heaps.Where(heap => !heap.Own && heap.Empty).OrderBy(heap => heap.EmptySince).ToList();

        foreach (var heap in empty.Skip(KeptEmptyHeaps).Where(heap => now - heap.EmptySince >= EmptyMilliseconds))
        {
            _heaps.Remove(heap);
            ComObject.Release(heap.Desktop);
            _returned++;
        }
    }

    /// <summary>How long a heap stays empty before it may go back (<see cref="ReturnEmpty"/>); for tests.</summary>
    internal long EmptyMilliseconds { get; set; } = DefaultEmptyMilliseconds;

    private static nint CreateHeap(nint device, ulong bytes, ulong alignment)
    {
        var desc = new HeapDescription
        {
            SizeInBytes = bytes,
            Properties = new HeapProperties { Type = DefaultHeapType },
            Alignment = alignment,
            Flags = NonTargetTexturesOnly,
        };
        Guid iid = HeapIid;
        nint heap = 0;
        int hr = ((delegate* unmanaged<nint, HeapDescription*, Guid*, nint*, int>)ComObject.Slot(device, CreateHeapSlot))(device, &desc, &iid, &heap);

        if (HResult.Failed(hr))
        {
            return 0;
        }

        return heap;
    }

    /// <summary>Sets an owner object as the resource's private data: the runtime releases it when the resource is destroyed.</summary>
    private void Own(nint resource, Heap heap, ulong offset, ulong size)
    {
        long id = Interlocked.Increment(ref _nextOwner);
        Owned[id] = (this, heap, offset, size);
        var owner = (OwnerObject*)NativeMemory.Alloc((nuint)sizeof(OwnerObject));
        *owner = new OwnerObject { Methods = OwnerVtable, References = 1, Id = id };

        Guid key = OwnerKey;
        _ = ((delegate* unmanaged<nint, Guid*, OwnerObject*, int>)ComObject.Slot(resource, SetPrivateDataInterfaceSlot))(resource, &key, owner);

        // The runtime holds its own reference now.
        Release(owner);
    }

    private void Freed(Heap heap, ulong offset, ulong size)
    {
        Interlocked.Increment(ref _freed);
        Give(heap, offset, size);
    }

    private static OwnerMethods* CreateOwnerVtable()
    {
        var methods = (OwnerMethods*)NativeMemory.Alloc((nuint)sizeof(OwnerMethods));
        methods->QueryInterface = &OwnerQueryInterface;
        methods->AddRef = &OwnerAddRef;
        methods->Release = &OwnerRelease;

        return methods;
    }

    [UnmanagedCallersOnly]
    private static int OwnerQueryInterface(OwnerObject* self, Guid* iid, void** result)
    {
        try
        {
            if (*iid == UnknownIid)
            {
                *result = self;
                Interlocked.Increment(ref self->References);

                return HResult.Ok;
            }

            *result = null;

            return HResult.NoInterface;
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "PlacedTextureHeaps.OwnerQueryInterface");

            return HResult.NoInterface;
        }
    }

    [UnmanagedCallersOnly]
    private static uint OwnerAddRef(OwnerObject* self)
    {
        try
        {
            return (uint)Interlocked.Increment(ref self->References);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "PlacedTextureHeaps.OwnerAddRef");

            return 1;
        }
    }

    [UnmanagedCallersOnly]
    private static uint OwnerRelease(OwnerObject* self)
    {
        try
        {
            return Release(self);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "PlacedTextureHeaps.OwnerRelease");

            return 0;
        }
    }

    /// <summary>The last release gives the texture's range back.</summary>
    private static uint Release(OwnerObject* self)
    {
        long count = Interlocked.Decrement(ref self->References);

        if (count != 0)
        {
            return (uint)count;
        }

        if (Owned.TryRemove(self->Id, out var owned))
        {
            owned.Heaps.Freed(owned.Heap, owned.Offset, owned.Size);
        }

        NativeMemory.Free(self);

        return 0;
    }

    /// <summary>D3D12_HEAP_DESC (d3d12.h).</summary>
    [StructLayout(LayoutKind.Sequential)]
    private struct HeapDescription
    {
        public ulong SizeInBytes;
        public HeapProperties Properties;
        public ulong Alignment;
        public int Flags;
    }

    /// <summary>The IUnknown methods of the owner object.</summary>
    [StructLayout(LayoutKind.Sequential)]
    private struct OwnerMethods
    {
        public delegate* unmanaged<OwnerObject*, Guid*, void**, int> QueryInterface;
        public delegate* unmanaged<OwnerObject*, uint> AddRef;
        public delegate* unmanaged<OwnerObject*, uint> Release;
    }

    /// <summary>The owner object: its methods, references and the id of the range it gives back.</summary>
    [StructLayout(LayoutKind.Sequential)]
    private struct OwnerObject
    {
        public OwnerMethods* Methods;
        public long References;
        public long Id;
    }

    /// <summary>A desktop heap and its free ranges, sorted by offset.</summary>
    private sealed class Heap(nint desktop, ulong bytes, bool own)
    {
        private readonly List<(ulong Offset, ulong Size)> _free = [(0, bytes)];

        public nint Desktop { get; } = desktop;

        public ulong Size { get; } = bytes;

        /// <summary>Whether the heap holds one texture only and goes with it.</summary>
        public bool Own { get; } = own;

        /// <summary>Bytes in ranges handed out.</summary>
        public ulong Used => Size - _free.Aggregate(0UL, (total, range) => total + range.Size);

        /// <summary>Whether every range is back.</summary>
        public bool Empty => _free.Count == 1 && _free[0].Size == Size;

        /// <summary>When the heap last became empty (Environment.TickCount64).</summary>
        public long EmptySince { get; private set; }

        public bool TryTake(ulong size, ulong alignment, out ulong offset)
        {
            for (int i = 0; i < _free.Count; i++)
            {
                var (start, length) = _free[i];
                ulong aligned = (start + alignment - 1) & ~(alignment - 1);

                if (aligned + size > start + length)
                {
                    continue;
                }

                _free.RemoveAt(i);

                if (aligned + size < start + length)
                {
                    _free.Insert(i, (aligned + size, start + length - aligned - size));
                }

                if (aligned > start)
                {
                    _free.Insert(i, (start, aligned - start));
                }

                offset = aligned;

                return true;
            }

            offset = 0;

            return false;
        }

        public void Give(ulong offset, ulong size)
        {
            int index = 0;

            while (index < _free.Count && _free[index].Offset < offset)
            {
                index++;
            }

            _free.Insert(index, (offset, size));

            // Merge with the neighbours.
            if (index + 1 < _free.Count && _free[index].Offset + _free[index].Size == _free[index + 1].Offset)
            {
                _free[index] = (_free[index].Offset, _free[index].Size + _free[index + 1].Size);
                _free.RemoveAt(index + 1);
            }

            if (index > 0 && _free[index - 1].Offset + _free[index - 1].Size == _free[index].Offset)
            {
                _free[index - 1] = (_free[index - 1].Offset, _free[index - 1].Size + _free[index].Size);
                _free.RemoveAt(index);
            }

            if (Empty)
            {
                EmptySince = Environment.TickCount64;
            }
        }
    }
}
