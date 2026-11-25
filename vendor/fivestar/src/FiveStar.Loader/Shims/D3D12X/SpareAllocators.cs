// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.D3D12X;

/// <summary>
/// Command allocators lent to a list whose Reset the desktop refuses with E_INVALIDARG because the game's allocator
/// already backs another list that is recording: the game takes a list and an allocator from separate pools
/// (0x140188430) and the console lets lists record on one allocator together; a failed Reset breaks into the debugger
/// at 0x142474ce1. Each game allocator keeps its own spares, reset with it (the game resets an allocator once the GPU
/// is done with every list recorded on it), so a spare is reused, not leaked. A spare not lent over its allocator's last
/// <see cref="IdleResets"/> resets is released instead of reset: an allocator keeps the memory of its largest recording,
/// charged to commit, and a spare lent once at a load would hold it for the rest of the run.
/// </summary>
internal sealed unsafe class SpareAllocators
{
    /// <summary>ID3D12DeviceChild::GetDevice, ID3D12CommandList::GetType, ID3D12Device::CreateCommandAllocator, ID3D12CommandAllocator::Reset (d3d12.h).</summary>
    private const int GetDeviceSlot = 7;
    private const int GetTypeSlot = 8;
    private const int CreateCommandAllocatorSlot = 9;
    private const int CreateCommandListSlot = 12;
    private const int AllocatorResetSlot = 8;

    private static readonly Guid DeviceIid = new("189819f1-1db6-4b57-be54-1821339b85f7");
    private static readonly Guid AllocatorIid = new("6102dee4-af59-4b09-b999-b44d73f09b24");

    /// <summary>Resets of its game allocator after which a spare not lent since is released.</summary>
    public const long IdleResets = 120;

    private readonly object _gate = new();
    private readonly Dictionary<nint, Pool> _spares = [];

    /// <summary>Lists that recorded on a spare.</summary>
    public long Lent { get; private set; }

    /// <summary>
    /// Resets desktop <paramref name="list"/> on a spare of desktop <paramref name="allocator"/> through
    /// <paramref name="reset"/> (ID3D12GraphicsCommandList::Reset): the first spare that takes it, else a new one.
    /// </summary>
    public int Reset(nint list, nint allocator, nint pipeline, delegate* unmanaged<nint, nint, nint, int> reset)
    {
        lock (_gate)
        {
            var pool = PoolOf(allocator);
            Lent++;

            foreach (var spare in pool.Spares)
            {
                if (!HResult.Failed(reset(list, spare.Allocator, pipeline)))
                {
                    spare.Lent = pool.Resets;

                    return HResult.Ok;
                }
            }

            nint created = Create(list);
            int hr = HResult.InvalidArgument;

            if (created != 0)
            {
                pool.Spares.Add(new Spare(created, pool.Resets));
                hr = reset(list, created, pipeline);
            }

            return hr;
        }
    }

    /// <summary>
    /// A new desktop list of <paramref name="list"/>'s type, recording on a new spare of desktop
    /// <paramref name="allocator"/>, for a list whose Close failed: the desktop refuses every later Reset of such a list
    /// with E_INVALIDARG, on any allocator. The old list is kept, not released, as the GPU may still run it. 0 when the
    /// desktop refuses.
    /// </summary>
    public nint Replace(nint list, nint allocator, nint pipeline)
    {
        lock (_gate)
        {
            nint spare = Create(list);

            if (spare == 0)
            {
                return 0;
            }

            var pool = PoolOf(allocator);
            pool.Spares.Add(new Spare(spare, pool.Resets));
            Guid deviceIid = DeviceIid;
            nint device = 0;

            if (HResult.Failed(((delegate* unmanaged<nint, Guid*, nint*, int>)ComObject.Slot(list, GetDeviceSlot))(list, &deviceIid, &device)))
            {
                return 0;
            }

            int type = ((delegate* unmanaged<nint, int>)ComObject.Slot(list, GetTypeSlot))(list);
            Guid listIid = D3D12XLayout.GraphicsCommandListIid;
            nint created = 0;
            int hr = ((delegate* unmanaged<nint, uint, int, nint, nint, Guid*, nint*, int>)ComObject.Slot(device, CreateCommandListSlot))(device, 0, type, spare, pipeline, &listIid, &created);
            ComObject.Release(device);

            Replaced++;
            ShimContext.Runtime.D3D12X.Warnings.Warn("list-replaced", $"[d3d12_xs] command list of type {type} refused every Reset after a failed Close: replaced by a new list ({HResult.Format(hr)})");

            if (HResult.Failed(hr))
            {
                return 0;
            }

            return created;
        }
    }

    /// <summary>Lists replaced after a failed Close.</summary>
    public long Replaced { get; private set; }

    /// <summary>Spares released after <see cref="IdleResets"/> resets of their allocator without a lend.</summary>
    public long Released { get; private set; }

    /// <summary>Spares held for desktop <paramref name="allocator"/>.</summary>
    public int Held(nint allocator)
    {
        lock (_gate)
        {
            if (!_spares.TryGetValue(allocator, out var pool))
            {
                return 0;
            }

            return pool.Spares.Count;
        }
    }

    /// <summary>
    /// Resets the spares of desktop <paramref name="allocator"/>, the GPU done with every list recorded on it; one a list
    /// still records on keeps its memory, and one idle for <see cref="IdleResets"/> resets is released.
    /// </summary>
    public void ResetSpares(nint allocator)
    {
        lock (_gate)
        {
            if (!_spares.TryGetValue(allocator, out var pool))
            {
                return;
            }

            pool.Resets++;
            int released = pool.Spares.RemoveAll(spare => ResetOrRelease(spare, pool.Resets));
            Released += released;
            CommandObjectCounts.SparesReleased(released);
        }
    }

    /// <summary>Resets <paramref name="spare"/>; true when it was idle, reset and released.</summary>
    private static bool ResetOrRelease(Spare spare, long resets)
    {
        int hr = ((delegate* unmanaged<nint, int>)ComObject.Slot(spare.Allocator, AllocatorResetSlot))(spare.Allocator);

        if (HResult.Failed(hr) || resets - spare.Lent < IdleResets)
        {
            return false;
        }

        ComObject.Release(spare.Allocator);

        return true;
    }

    private Pool PoolOf(nint allocator)
    {
        if (!_spares.TryGetValue(allocator, out var pool))
        {
            pool = new Pool();
            _spares[allocator] = pool;
        }

        return pool;
    }

    /// <summary>A new allocator of <paramref name="list"/>'s type on its device; 0 when the desktop refuses.</summary>
    private static nint Create(nint list)
    {
        Guid deviceIid = DeviceIid;
        nint device = 0;

        if (HResult.Failed(((delegate* unmanaged<nint, Guid*, nint*, int>)ComObject.Slot(list, GetDeviceSlot))(list, &deviceIid, &device)))
        {
            return 0;
        }

        int type = ((delegate* unmanaged<nint, int>)ComObject.Slot(list, GetTypeSlot))(list);
        Guid allocatorIid = AllocatorIid;
        nint allocator = 0;
        int hr = ((delegate* unmanaged<nint, int, Guid*, nint*, int>)ComObject.Slot(device, CreateCommandAllocatorSlot))(device, type, &allocatorIid, &allocator);
        ComObject.Release(device);

        if (HResult.Failed(hr))
        {
            return 0;
        }

        CommandObjectCounts.SpareMade();

        return allocator;
    }

    /// <summary>A spare and its allocator's reset count when it was last lent.</summary>
    private sealed class Spare(nint allocator, long lent)
    {
        public nint Allocator { get; } = allocator;

        public long Lent { get; set; } = lent;
    }

    /// <summary>One game allocator's spares and its resets so far.</summary>
    private sealed class Pool
    {
        public List<Spare> Spares { get; } = [];

        public long Resets { get; set; }
    }
}
