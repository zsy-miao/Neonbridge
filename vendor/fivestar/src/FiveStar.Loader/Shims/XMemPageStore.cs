// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Diagnostics.CodeAnalysis;
using System.Runtime.InteropServices;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims;

/// <summary>
/// The console's physical memory: 64 KiB pages carved from pagefile-backed sections of <see cref="ChunkBytes"/> (or a
/// multiple, for a larger request), created as pages are first handed out. A request for several pages gets consecutive
/// pages of one section. A page is a fixed offset in one section, so every view of it shares its contents.
/// Page frame numbers count 4 KiB frames like the console's (page index times 16, never 0). Allocation and freeing
/// lock; locating a frame does not and never allocates.
/// </summary>
internal sealed unsafe class XMemPageStore : IDisposable
{
    /// <summary>A physical page: 64 KiB (the GDK memory overview), which is also the Windows allocation granularity.</summary>
    public const ulong PageSize = 0x10000;

    /// <summary>Frames per page: the game turns a frame number into a page index with bits 4..35 (bextr 0x2004).</summary>
    public const ulong FramesPerPage = PageSize / Win32Constants.PageSize;

    /// <summary>Pages per section slot: 64 MiB of commit charge each time the store grows.</summary>
    private const ulong PagesPerChunk = 1024;

    private const ulong ChunkBytes = PagesPerChunk * PageSize;

    /// <summary>The game packs page indices into 21-bit fields (mask 0x1FFFFF): at most 2^21 pages, 128 GiB.</summary>
    private const int PageIndexBits = 21;

    private const ulong MaxPages = 1UL << PageIndexBits;

    private const int MaxChunks = (int)(MaxPages / PagesPerChunk);

    /// <summary>Page 0 is never handed out, so frame 0 can mean "no page" as in a scatter list.</summary>
    private const ulong FirstPage = 1;

    /// <summary>The remap entry of a group page whose backing is not chosen yet; 0 means the page backs itself.</summary>
    private const uint Unbound = uint.MaxValue;

    private readonly object _gate = new();
    private readonly XMemPageChunk?[] _chunks = new XMemPageChunk?[MaxChunks];
    private readonly uint[]?[] _remap = new uint[]?[MaxChunks];
    private readonly List<XMemPageGroup> _groups = [];
    private readonly nint _process;
    private ulong[] _freed = [];
    private int _freedCount;
    private ulong _nextPage = FirstPage;
    private ulong _backedPages = FirstPage;
    private long _livePages;

    public XMemPageStore(nint process)
    {
        _process = process;
    }

    public long LivePages => Interlocked.Read(ref _livePages);

    /// <summary>Fills <paramref name="frames"/> with up to <paramref name="count"/> zeroed pages; returns how many it got.</summary>
    public ulong Allocate(ulong count, ulong* frames)
    {
        ulong allocated = 0;

        lock (_gate)
        {
            if (count > 1 && TryTakeContiguous(count, out ulong first))
            {
                AddGroup(first, count);

                for (; allocated < count; allocated++)
                {
                    frames[allocated] = (first + allocated) * FramesPerPage;
                }
            }

            while (allocated < count && TryTakePage(out ulong page))
            {
                frames[allocated] = page * FramesPerPage;
                allocated++;
            }
        }

        Interlocked.Add(ref _livePages, (long)allocated);

        return allocated;
    }

    /// <summary>Returns pages to the store, zeroing them; frames the store never handed out are skipped.</summary>
    public void Free(ulong count, ulong* frames)
    {
        long freed = 0;

        lock (_gate)
        {
            for (ulong i = 0; i < count; i++)
            {
                // A frame freed twice would overfill the free list; skip it.
                if (_freedCount == _freed.Length || !TryLocate(frames[i], out var chunk, out ulong offset))
                {
                    continue;
                }

                NativeMemory.Clear((void*)(chunk.Window + (nint)offset), (nuint)PageSize);
                _freed[_freedCount++] = frames[i] / FramesPerPage;
                freed++;
            }
        }

        Interlocked.Add(ref _livePages, -freed);
    }

    /// <summary>
    /// The section and byte offset holding frame <paramref name="frame"/>, if the store handed it out. The first call
    /// for a frame of a multi-page allocation binds it to a page of its group (<see cref="XMemPageGroup"/>).
    /// </summary>
    public bool TryLocate(ulong frame, [NotNullWhen(true)] out XMemPageChunk? chunk, out ulong offset)
    {
        return TryLocate(frame, 0, 0, out chunk, out offset);
    }

    /// <summary>
    /// <see cref="TryLocate(ulong, out XMemPageChunk?, out ulong)"/> for a frame being mapped at <paramref name="address"/>
    /// in a region starting at <paramref name="origin"/>, which places a group frame by its address.
    /// </summary>
    public bool TryLocate(ulong frame, nint address, nint origin, [NotNullWhen(true)] out XMemPageChunk? chunk, out ulong offset)
    {
        ulong page = frame / FramesPerPage;
        chunk = null;
        offset = 0;

        if (frame % FramesPerPage != 0 || page < FirstPage || page >= Volatile.Read(ref _backedPages))
        {
            return false;
        }

        ulong backing = Backing(page, address, origin);
        var found = Volatile.Read(ref _chunks[(backing - FirstPage) / PagesPerChunk]);

        if (found is null)
        {
            return false;
        }

        chunk = found;
        offset = (backing - found.FirstPage) * PageSize;

        return true;
    }

    public void Dispose()
    {
        XMemPageChunk? previous = null;

        foreach (var chunk in _chunks)
        {
            // A section larger than one slot fills consecutive slots.
            if (chunk is null || ReferenceEquals(chunk, previous))
            {
                continue;
            }

            previous = chunk;

            _ = KernelBase.UnmapViewOfFile2(_process, chunk.Window, 0);
            _ = Kernel32.CloseHandle(chunk.Section);
        }

        Array.Clear(_chunks);
    }

    /// <summary>The page backing <paramref name="page"/>: itself, or its group's page, bound now if it has none.</summary>
    private ulong Backing(ulong page, nint address, nint origin)
    {
        ulong index = page - FirstPage;
        uint[]? block = Volatile.Read(ref _remap[index / PagesPerChunk]);

        if (block is null)
        {
            return page;
        }

        uint entry = Volatile.Read(ref block[index % PagesPerChunk]);

        if (entry == Unbound)
        {
            return Bind(page, block, address, origin);
        }

        if (entry == 0)
        {
            return page;
        }

        return entry;
    }

    private ulong Bind(ulong page, uint[] block, nint address, nint origin)
    {
        ulong slot = (page - FirstPage) % PagesPerChunk;

        lock (_gate)
        {
            uint entry = block[slot];

            if (entry != Unbound)
            {
                return entry;
            }

            ulong backing = FindGroup(page).Take(address, origin);
            Volatile.Write(ref block[slot], (uint)backing);

            return backing;
        }
    }

    /// <summary>The group holding <paramref name="page"/>; groups are added in page order.</summary>
    private XMemPageGroup FindGroup(ulong page)
    {
        int low = 0;
        int high = _groups.Count - 1;

        while (low < high)
        {
            int middle = low + ((high - low + 1) >> 1);

            if (_groups[middle].FirstPage <= page)
            {
                low = middle;
            }
            else
            {
                high = middle - 1;
            }
        }

        return _groups[low];
    }

    /// <summary>Records a multi-page allocation; its frames are bound to its pages as they are first mapped.</summary>
    private void AddGroup(ulong first, ulong count)
    {
        _groups.Add(new XMemPageGroup(first, count));

        for (ulong page = first; page < first + count; page++)
        {
            ulong index = page - FirstPage;
            ref uint[]? block = ref _remap[index / PagesPerChunk];

            if (block is null)
            {
                Volatile.Write(ref block, new uint[PagesPerChunk]);
            }

            block![index % PagesPerChunk] = Unbound;
        }
    }

    private bool TryTakePage(out ulong page)
    {
        if (_freedCount > 0)
        {
            page = _freed[--_freedCount];

            return true;
        }

        page = _nextPage;

        if (page >= _backedPages && !TryAddChunk(PagesPerChunk))
        {
            return false;
        }

        _nextPage++;

        return true;
    }

    /// <summary>
    /// <paramref name="count"/> consecutive fresh pages of one section, so the D3D12 shim can place one buffer over a
    /// range the game maps from one allocation. When the current section has too few left, its rest goes to the free
    /// list and a section of at least <paramref name="count"/> pages is added.
    /// </summary>
    private bool TryTakeContiguous(ulong count, out ulong first)
    {
        first = _nextPage;

        if (_backedPages - _nextPage < count)
        {
            ulong pages = (count + PagesPerChunk - 1) / PagesPerChunk * PagesPerChunk;
            ulong rest = _backedPages - _nextPage;

            if (!TryAddChunk(pages))
            {
                return false;
            }

            for (ulong i = 0; i < rest; i++)
            {
                _freed[_freedCount++] = _nextPage + i;
            }

            first = _backedPages - pages;
        }

        _nextPage = first + count;

        return true;
    }

    /// <summary>A section of <paramref name="pages"/> pages (a multiple of <see cref="PagesPerChunk"/>) after the last one.</summary>
    private bool TryAddChunk(ulong pages)
    {
        int index = (int)((_backedPages - FirstPage) / PagesPerChunk);
        int slots = (int)(pages / PagesPerChunk);

        if (index + slots > MaxChunks)
        {
            return false;
        }

        ulong bytes = pages * PageSize;
        nint section = KernelBase.CreateFileMapping2(MemoryConstants.InvalidHandleValue, null, MemoryConstants.FileMapRead | MemoryConstants.FileMapWrite, Win32Constants.PageReadWrite, MemoryConstants.SecCommit, bytes, null, null, 0);

        if (section == 0)
        {
            return false;
        }

        nint window = KernelBase.MapViewOfFile3(section, _process, 0, 0, (nuint)bytes, 0, Win32Constants.PageReadWrite, null, 0);

        if (window == 0)
        {
            _ = Kernel32.CloseHandle(section);

            return false;
        }

        Array.Resize(ref _freed, _freed.Length + (int)pages);
        var chunk = new XMemPageChunk(section, window, _backedPages, pages);

        for (int slot = index; slot < index + slots; slot++)
        {
            Volatile.Write(ref _chunks[slot], chunk);
        }

        Volatile.Write(ref _backedPages, _backedPages + pages);

        return true;
    }
}
