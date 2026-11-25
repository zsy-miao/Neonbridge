// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Diagnostics.CodeAnalysis;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims;

/// <summary>
/// The xmem.dll virtual and physical page functions. Physical pages live in pagefile-backed sections
/// (<see cref="XMemPageStore"/>); a MEM_64K_PAGES reservation is a placeholder range (<see cref="XMemReservation"/>) and
/// mapping a page replaces one 64 KiB placeholder with a view of the page's section. A page mapped at two addresses
/// therefore aliases, and a remapped page keeps its contents. Signatures and flags are in docs/loader.md.
/// </summary>
internal sealed unsafe class XMemPhysicalMemory : IDisposable
{
    /// <summary>The mapping unit (see <see cref="XMemPageStore.PageSize"/>).</summary>
    public const ulong PageSize = XMemPageStore.PageSize;

    /// <summary>The standard PAGE_* protection; the bits above are cache and PAGE_GRAPHICS_* (GPU) attributes.</summary>
    private const uint StandardProtectionMask = 0xFF;

    private readonly LoaderLog _log;
    private readonly nint _process;
    private readonly XMemPageStore _pages;
    private readonly XMemReservationTable _reservations = new();
    private readonly List<(nint Address, nuint Size)> _placeholders = [];
    private XMemReservation? _lastCpuReservation;
    private Action<nint, ulong, ulong>? _pageChanged;

    public XMemPhysicalMemory(LoaderLog log)
    {
        _log = log;
        _process = Kernel32.GetCurrentProcess();
        _pages = new XMemPageStore(_process);
    }

    public long LivePages => _pages.LivePages;

    /// <summary>
    /// The bases XMemVirtualAlloc gave out as plain VirtualAlloc memory, for the memory map (--memory-map); the game
    /// frees them with its own VirtualFree, so a base may be stale.
    /// </summary>
    public AllocationBases PlainAllocations { get; } = new();

    /// <summary>The GPU access of those allocations, from their flags.</summary>
    public GraphicsAllocations Graphics { get; } = new();

    /// <summary>
    /// <c>void* XMemVirtualAlloc(void* address, SIZE_T size, DWORD allocationType, uint64 xmemFlags, DWORD protect)</c>.
    /// MEM_RESERVE_PLACEHOLDER makes a placeholder the game splits itself; MEM_64K_PAGES without MEM_COMMIT makes a
    /// mappable reservation (in an existing placeholder with MEM_REPLACE_PLACEHOLDER); anything else is VirtualAlloc
    /// with MEM_RESERVE/MEM_COMMIT and the low protection byte.
    /// </summary>
    public nint VirtualAlloc(nint address, nuint size, uint allocationType, ulong xmemFlags, uint protect)
    {
        nint result;

        if ((allocationType & MemoryConstants.MemReservePlaceholder) != 0)
        {
            result = ReserveGamePlaceholder(address, size);
        }
        else if ((allocationType & MemoryConstants.MemPhysical) != 0 && (allocationType & Win32Constants.MemCommit) == 0)
        {
            result = ReserveMappable(address, size, allocationType, protect);
        }
        else
        {
            result = Allocate(address, size, allocationType, protect);
            PlainAllocations.Add(result);
            Graphics.Add(result, size, allocationType, protect);
        }

        if (_log.DetailEnabled)
        {
            _log.Detail($"  [xmem] XMemVirtualAlloc(0x{address:x}, 0x{size:x}, type=0x{allocationType:x}, flags=0x{xmemFlags:x}, protect=0x{protect:x}) -> 0x{result:x}");
        }

        return result;
    }

    /// <summary><c>BOOL XMemAllocatePhysicalPages(DWORD flags, ULONG_PTR* count, ULONG_PTR* frames)</c>: *count becomes the number allocated.</summary>
    public int AllocatePages(ulong flags, ulong* count, ulong* frames)
    {
        if (count == null || frames == null)
        {
            return 0;
        }

        ulong requested = *count;
        *count = _pages.Allocate(requested, frames);
        _log.Detail($"  [xmem] XMemAllocatePhysicalPages(flags=0x{flags:x}, count={requested}) -> {*count}");

        if (*count != requested)
        {
            _log.Warn($"[xmem] XMemAllocatePhysicalPages: only {*count} of {requested} pages");
        }

        return 1;
    }

    /// <summary><c>BOOL XMemFreePhysicalPages(ULONG_PTR count, ULONG_PTR* frames)</c>.</summary>
    public int FreePages(ulong count, ulong* frames)
    {
        if (frames == null)
        {
            return 0;
        }

        _pages.Free(count, frames);

        return 1;
    }

    /// <summary>
    /// <c>void* XMemMapPhysicalPages(void* address, ULONG_PTR count, ULONG_PTR* frames)</c>: returns the address, or 0
    /// on failure. Pages already mapped are replaced; a null array unmaps and keeps the reservation.
    /// </summary>
    public nint MapPages(nint address, ulong count, ulong* frames)
    {
        bool mapped = true;

        for (ulong i = 0; i < count; i++)
        {
            ulong frame = frames == null ? 0 : frames[i];
            mapped &= MapOne(address + (nint)(i * PageSize), frame);
        }

        if (frames == null)
        {
            CoalesceIfWhole(address, count * PageSize);
        }

        if (!mapped)
        {
            _log.Warn($"[xmem] XMemMapPhysicalPages(0x{address:x}, {count} pages) failed");

            return 0;
        }

        return address;
    }

    /// <summary>
    /// <c>BOOL XMemMapPhysicalPagesScatter(void** addresses, ULONG_PTR count, ULONG_PTR* frames)</c>: one page per
    /// address; a 0 entry or a null array unmaps. The game breaks into the debugger when this returns FALSE.
    /// </summary>
    public int MapPagesScatter(nint* addresses, ulong count, ulong* frames)
    {
        if (addresses == null)
        {
            return 0;
        }

        bool mapped = true;

        for (ulong i = 0; i < count; i++)
        {
            ulong frame = frames == null ? 0 : frames[i];
            mapped &= MapOne(addresses[i], frame);
        }

        if (!mapped)
        {
            _log.Warn($"[xmem] XMemMapPhysicalPagesScatter({count} pages): a mapping failed");

            return 0;
        }

        return 1;
    }

    /// <summary>
    /// D3D12.X queue CopyPageMappingsBatchX (<see cref="D3D12X.QueuePageMappings"/>): maps pages of a pool (a range the
    /// game mapped physical pages into) at each batch's destination, range by range: kind 0 the pool pages from
    /// <c>Start</c> in order, 1 pool page <c>Start</c> at every page, 2 unmapped, 3 left as they are. False when a range
    /// names pages past the pool or a mapping fails; the other ranges are still mapped.
    /// </summary>
    public bool CopyPageMappings(uint count, PageBatch* batches, nint pool, uint poolPages)
    {
        bool mapped = true;
        nint poolCpu = CpuAddress(pool);

        for (uint b = 0; b < count && batches != null; b++)
        {
            var batch = batches[b];
            nint at = (nint)batch.Destination;

            for (uint r = 0; r < batch.Count && batch.Ranges != null; r++)
            {
                var range = batch.Ranges[r];
                mapped &= MapRange(at, range, poolCpu, poolPages);
                at += (nint)(range.Count * PageSize);
            }
        }

        if (!mapped)
        {
            _log.Warn("[xmem] CopyPageMappingsBatchX: a page mapping failed");
        }

        return mapped;
    }

    private bool MapRange(nint destination, PageRange range, nint pool, uint poolPages)
    {
        bool mapped = true;

        for (uint k = 0; k < range.Count; k++)
        {
            nint at = destination + (nint)(k * PageSize);

            switch (range.Kind)
            {
                case PageRange.PoolPages when (ulong)range.Start + range.Count <= poolPages:
                    mapped &= MapOne(at, FrameAt(pool + (nint)((range.Start + k) * PageSize)));
                    break;
                case PageRange.OnePoolPage when range.Start < poolPages:
                    mapped &= MapOne(at, FrameAt(pool + (nint)(range.Start * PageSize)));
                    break;
                case PageRange.Unmapped:
                    mapped &= MapOne(at, 0);
                    break;
                case PageRange.Unchanged:
                    break;
                default:
                    return false;
            }
        }

        return mapped;
    }

    /// <summary>The frame mapped at <paramref name="address"/>; 0 when none is.</summary>
    private ulong FrameAt(nint address)
    {
        return _reservations.Find(address)?.FrameAt(address) ?? 0;
    }

    /// <summary>
    /// The section and byte offset of the page mapped at <paramref name="address"/>: what a GPU heap over the same
    /// memory needs. False when nothing is mapped there.
    /// </summary>
    public bool TryLocate(nint address, [NotNullWhen(true)] out XMemPageChunk? chunk, out ulong offset)
    {
        chunk = null;
        offset = 0;
        address = CpuAddress(address);
        var reservation = _reservations.Find(address);

        if (reservation is null || !_pages.TryLocate(reservation.FrameAt(address), out chunk, out ulong pageOffset))
        {
            return false;
        }

        offset = pageOffset + ((ulong)(address - reservation.Address) % PageSize);

        return true;
    }

    /// <summary>
    /// Copies <paramref name="data"/> into physical pages by frame number, as the console's DirectStorage does for a
    /// page-list destination: from byte <paramref name="offset"/> of the page <paramref name="frames"/>[0] names, on
    /// through the next frames one 64 KiB page each. False when a frame is not one the store handed out.
    /// </summary>
    public bool WriteFrames(ulong* frames, ulong offset, ReadOnlySpan<byte> data)
    {
        ulong index = offset / PageSize;
        ulong inPage = offset % PageSize;

        while (!data.IsEmpty)
        {
            if (!_pages.TryLocate(frames[index], out var chunk, out ulong pageOffset))
            {
                return false;
            }

            int length = (int)Math.Min((ulong)data.Length, PageSize - inPage);
            data[..length].CopyTo(new Span<byte>((void*)(chunk.Window + (nint)(pageOffset + inPage)), length));
            data = data[length..];
            inPage = 0;
            index++;
        }

        return true;
    }

    /// <summary>
    /// Where the CPU reads <paramref name="address"/>: the same offset in the paired CPU reservation for an address in
    /// a GPU-only alias (<see cref="XMemReservation.CpuView"/>), else the address itself.
    /// </summary>
    [HotPath]
    public nint CpuAddress(nint address)
    {
        var reservation = _reservations.Find(address);

        if (reservation is null)
        {
            return address;
        }

        var cpu = reservation.CpuView;

        if (cpu is null)
        {
            return address;
        }

        return cpu.Address + (address - reservation.Address);
    }

    /// <summary>
    /// Where the GPU addresses the page the CPU maps at <paramref name="address"/>: the same offset in the GPU-only
    /// alias of its reservation; 0 when it has none.
    /// </summary>
    public nint GpuAlias(nint address)
    {
        var reservation = _reservations.Find(address);
        var alias = reservation?.GpuAlias;

        if (reservation is null || alias is null)
        {
            return 0;
        }

        return alias.Address + (address - reservation.Address);
    }

    /// <summary>
    /// Adds <paramref name="changed"/> to the calls made (address, previous frame, new frame; 0 is none) every time the
    /// frame at a mapped address changes: a remap, an unmap or a first mapping, from the thread that made it.
    /// </summary>
    public void ObservePages(Action<nint, ulong, ulong> changed)
    {
        lock (_placeholders)
        {
            Volatile.Write(ref _pageChanged, (Action<nint, ulong, ulong>)Delegate.Combine(_pageChanged, changed));
        }
    }

    /// <summary>Calls <paramref name="visit"/> (address, frame) for every page mapped now, in address order per reservation.</summary>
    public void VisitMapped(Action<nint, ulong> visit)
    {
        foreach (var reservation in _reservations.Snapshot())
        {
            for (ulong offset = 0; offset < reservation.Size; offset += PageSize)
            {
                nint address = reservation.Address + (nint)offset;
                ulong frame = reservation.FrameAt(address);

                if (frame != 0)
                {
                    visit(address, frame);
                }
            }
        }
    }

    /// <summary>Whether a page is mapped at <paramref name="address"/> (after <see cref="CpuAddress"/>); false outside every reservation.</summary>
    [HotPath]
    public bool IsMapped(nint address)
    {
        var reservation = _reservations.Find(address);

        return reservation is not null && reservation.FrameAt(address) != 0;
    }

    /// <summary>Whether <paramref name="address"/> lies in a mappable reservation.</summary>
    [HotPath]
    public bool IsReserved(nint address)
    {
        return _reservations.Find(address) is not null;
    }

    public void Dispose()
    {
        foreach (var reservation in _reservations.TakeAll())
        {
            reservation.Release();
        }

        _pages.Dispose();
    }

    private bool MapOne(nint address, ulong frame)
    {
        var reservation = _reservations.Find(address);

        if (reservation is null)
        {
            return false;
        }

        ulong previous = reservation.FrameAt(address);
        bool mapped;

        if (frame == 0)
        {
            mapped = reservation.Unmap(address);
        }
        else
        {
            mapped = _pages.TryLocate(frame, address, reservation.Origin, out var chunk, out ulong offset) && reservation.Map(address, frame, chunk, offset);
        }

        if (mapped && previous != frame)
        {
            Volatile.Read(ref _pageChanged)?.Invoke(address, previous, frame);
        }

        return mapped;
    }

    private void CoalesceIfWhole(nint address, ulong size)
    {
        var reservation = _reservations.Find(address);

        if (reservation is not null && reservation.Address == address && reservation.Size == size)
        {
            reservation.Coalesce();
        }
    }

    /// <summary>A placeholder the game splits into reservations itself; remembered as their common origin.</summary>
    private nint ReserveGamePlaceholder(nint address, nuint size)
    {
        nint result = ReservePlaceholder(address, size);

        if (result != 0)
        {
            lock (_placeholders)
            {
                _placeholders.Add((result, size));
            }
        }

        return result;
    }

    /// <summary>The game placeholder holding <paramref name="address"/> (the last one made there), else the address.</summary>
    private nint OriginOf(nint address)
    {
        lock (_placeholders)
        {
            for (int i = _placeholders.Count - 1; i >= 0; i--)
            {
                var (start, size) = _placeholders[i];

                if ((nuint)(address - start) < size)
                {
                    return start;
                }
            }
        }

        return address;
    }

    private nint ReservePlaceholder(nint address, nuint size)
    {
        return KernelBase.VirtualAlloc2(_process, address, size, Win32Constants.MemReserve | MemoryConstants.MemReservePlaceholder, Win32Constants.PageNoAccess, null, 0);
    }

    private nint ReserveMappable(nint address, nuint size, uint allocationType, uint protect)
    {
        nint result = address;

        // With MEM_REPLACE_PLACEHOLDER the range is already a placeholder the game split off; it stays one until mapped.
        if ((allocationType & MemoryConstants.MemReplacePlaceholder) == 0)
        {
            result = ReservePlaceholder(address, size);
        }

        if (result != 0)
        {
            var reservation = new XMemReservation(_process, result, size, ViewProtection(protect), OriginOf(result));
            PairGpuAlias(reservation, protect);
            _reservations.Add(reservation);
        }

        return result;
    }

    /// <summary>
    /// The game reserves its streaming range twice, a CPU range (protect PAGE_READWRITE) and then a GPU-only range of
    /// the same size (PAGE_NOACCESS | PAGE_GRAPHICS_NOACCESS, 0x141E6C1A6 and 0x141E6C1CB), and maps pages only into
    /// the first; its GPU addresses are CPU address - CPU base + GPU base (0x144ECC8F0). Such a second range becomes
    /// an alias of the first.
    /// </summary>
    private void PairGpuAlias(XMemReservation reservation, uint protect)
    {
        bool gpuOnly = (protect & StandardProtectionMask) == Win32Constants.PageNoAccess && (protect & ~StandardProtectionMask) != 0;
        var previous = _lastCpuReservation;

        if (!gpuOnly)
        {
            _lastCpuReservation = reservation;

            return;
        }

        if (previous is not null && previous.Size == reservation.Size)
        {
            reservation.CpuView = previous;
            previous.GpuAlias = reservation;
            _log.Detail($"  [xmem] 0x{reservation.Address:x} (GPU only) aliases the pages of 0x{previous.Address:x}");
        }

        _lastCpuReservation = null;
    }

    private nint Allocate(nint address, nuint size, uint allocationType, uint protect)
    {
        uint type = allocationType & Win32Constants.MemReserveCommit;

        if (address == 0 || type == 0)
        {
            type |= Win32Constants.MemReserve;
        }

        uint protection = protect & StandardProtectionMask;

        if (protection == 0)
        {
            protection = Win32Constants.PageReadWrite;
        }

        if ((allocationType & MemoryConstants.MemReplacePlaceholder) != 0)
        {
            return KernelBase.VirtualAlloc2(_process, address, size, type | MemoryConstants.MemReplacePlaceholder, protection, null, 0);
        }

        return Kernel32.VirtualAlloc(address, size, type, protection);
    }

    /// <summary>
    /// Views are read-only when the CPU protection is PAGE_READONLY and read-write otherwise, PAGE_NOACCESS included:
    /// the game reserves GPU-only ranges (PAGE_NOACCESS | PAGE_GRAPHICS_NOACCESS) that the D3D12 shim reaches through
    /// the CPU. Cache attributes (PAGE_WRITECOMBINE, PAGE_NOCACHE) cannot differ between views of one section and are dropped.
    /// </summary>
    private static uint ViewProtection(uint protect)
    {
        uint protection = (protect & StandardProtectionMask) == Win32Constants.PageReadOnly ? Win32Constants.PageReadOnly : Win32Constants.PageReadWrite;

        return protection;
    }
}
