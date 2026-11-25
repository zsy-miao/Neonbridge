// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.D3D12X;

/// <summary>
/// The desktop heaps over XMem page sections (ID3D12Device3::OpenExistingHeapFromAddress), so buffers placed on them
/// read and write the game's physical pages; used by slot-133 buffers and the address map alike. A section of at most
/// <see cref="WindowBytes"/> is one heap over the store's whole-section view. A larger one (the store makes one section
/// per contiguous allocation, and the view of a 6 GiB one is refused with E_INVALIDARG) takes a heap per window, each
/// through its own view of that part of the section, opened on the first request there.
/// <para>
/// Such heaps take the GPU's system-memory budget (<see cref="GpuMemoryBudget"/>, about half the machine's RAM), and
/// past it OpenExistingHeapFromAddress fails with E_OUTOFMEMORY, as do the upload buffers after it. So a window opens
/// only while it fits the room: the budget less the process's use and <see cref="Headroom"/>, and <see cref="Cap"/>
/// (<c>--section-heap-budget</c>). The address map's requests also leave <see cref="PlacedReserve"/> to placed buffers.
/// A section that does not fit whole at its first request is backed lazily, in windows of <see cref="LazyWindowBytes"/>.
/// A request that does not fit returns no heap and may succeed later; a heap the desktop refuses is never asked again.
/// </para>
/// </summary>
internal sealed unsafe class SectionHeaps
{
    /// <summary>Bytes of section one heap opens at most when the whole section fits.</summary>
    public const ulong WindowBytes = 1UL << 30;

    /// <summary>The window of a section backed lazily: a placed buffer opens only the 64 MiB around it.</summary>
    public const ulong LazyWindowBytes = 64UL << 20;

    /// <summary>System-memory budget left to everything else: texture uploads, the upload ring, readbacks.</summary>
    public const ulong Headroom = 768UL << 20;

    /// <summary>Room the address map's requests leave to placed buffers.</summary>
    public const ulong PlacedReserve = 1UL << 30;

    /// <summary>Room checks between two budget reads while no heap opens (DXGI is not read per page).</summary>
    private const int ReadInterval = 1024;

    private const ulong Mebibyte = 1UL << 20;

    /// <summary>ID3D12Device3::OpenExistingHeapFromAddress (d3d12.h).</summary>
    private const int OpenHeapFromAddressSlot = 48;

    private static readonly Guid Device3Iid = new("81dadc15-2bad-4392-93c5-101345c4aa98");
    private static readonly Guid HeapIid = new("6b3b2502-6e51-45b3-90ee-9884265e8df3");

    private readonly object _gate = new();
    private readonly Dictionary<(nint Section, ulong Start), SectionWindow> _all = [];
    private readonly List<SectionWindow> _byId = [];
    private readonly Dictionary<nint, ulong> _windows = [];
    private readonly HashSet<(nint Section, SectionHeapUse Use)> _refusals = [];
    private readonly Func<nint, nint, nint> _open;
    private readonly LoaderLog _log;
    private ulong _opened;
    private ulong _systemRoom = ulong.MaxValue;
    private int _sinceRead = ReadInterval;

    public SectionHeaps(LoaderLog log)
        : this(log, null)
    {
    }

    /// <param name="log">The loader log.</param>
    /// <param name="open">Opens a heap over a view (device, view; 0 on failure); null for the desktop call.</param>
    internal SectionHeaps(LoaderLog log, Func<nint, nint, nint>? open)
    {
        _log = log;
        _open = open ?? OpenOnDevice;
    }

    /// <summary>
    /// The largest heap opened in place over game memory (<c>--in-place-window</c>), a power of two from
    /// <see cref="LazyWindowBytes"/> to <see cref="WindowBytes"/>: sections and the game's own allocations larger than
    /// it open in windows of this size as they are used. <see cref="WindowBytes"/> by default (XCL users on NVIDIA lost
    /// the device at the world load with the game's memory open in place, and less so with a 512 MB window).
    /// </summary>
    public static ulong InPlaceWindow { get; set; } = WindowBytes;

    /// <summary><paramref name="megabytes"/> as an <see cref="InPlaceWindow"/>: rounded down to a power of two within its range.</summary>
    public static ulong InPlaceWindowOf(ulong megabytes)
    {
        ulong bytes = Math.Clamp(megabytes * Mebibyte, LazyWindowBytes, WindowBytes);

        return 1UL << System.Numerics.BitOperations.Log2(bytes);
    }

    /// <summary>The most bytes of section heaps open at once (<c>--section-heap-budget</c>); unlimited by default.</summary>
    public ulong Cap { get; set; } = ulong.MaxValue;

    /// <summary>The device's budget, read before windows open; null when there is none.</summary>
    public GpuMemoryBudget? Budget { get; set; }

    /// <summary>Called (section, window start, bytes) after a window's heap opened, outside the lock.</summary>
    public Action<nint, ulong, ulong>? WindowOpened { get; set; }

    /// <summary>Bytes of section heaps open.</summary>
    public ulong Opened
    {
        get
        {
            lock (_gate)
            {
                return _opened;
            }
        }
    }

    /// <summary>
    /// The heap holding byte <paramref name="offset"/> of <paramref name="chunk"/>'s section, opened now if it fits:
    /// <paramref name="heapOffset"/> is that byte's offset in its window and <paramref name="heapBytes"/> the window's
    /// size, set either way. 0 when the window does not fit (yet) or the desktop refused it.
    /// </summary>
    public nint Heap(nint device, XMemPageChunk chunk, ulong offset, SectionHeapUse use, out ulong heapOffset, out ulong heapBytes)
    {
        ulong sectionBytes = chunk.Pages * XMemPhysicalMemory.PageSize;
        ulong start;
        nint heap;

        lock (_gate)
        {
            ulong size = Window(chunk.Section, sectionBytes);
            start = offset / size * size;
            heapOffset = offset - start;
            heapBytes = Math.Min(size, sectionBytes - start);
            var window = Find(chunk, start, heapBytes);

            if (use == SectionHeapUse.Placed)
            {
                window.Pinned = true;
            }

            if (window.Heap != 0 || window.Failed || window.Retiring)
            {
                return window.Heap;
            }

            // With --evict-windows the GPU says which windows of a lazily backed section it reads (WindowResidency).
            if (use == SectionHeapUse.Map && size == LazyWindowBytes && Binding.WindowFeedback.Enabled)
            {
                return 0;
            }

            ulong room = Room(use);

            if (heapBytes > room)
            {
                Refuse(chunk.Section, start, use, room);

                return 0;
            }

            heap = OpenWindow(device, window, sectionBytes, use);

            if (heap == 0)
            {
                return 0;
            }
        }

        WindowOpened?.Invoke(chunk.Section, start, heapBytes);

        return heap;
    }

    /// <summary>The window of <paramref name="section"/> at <paramref name="start"/>, once some request named it.</summary>
    public SectionWindow? Find(nint section, ulong start)
    {
        lock (_gate)
        {
            return _all.GetValueOrDefault((section, start));
        }
    }

    /// <summary>The window with id <paramref name="id"/> (<see cref="SectionWindow.Id"/>), or null.</summary>
    public SectionWindow? ById(int id)
    {
        lock (_gate)
        {
            if (id <= 0 || id > _byId.Count)
            {
                return null;
            }

            return _byId[id - 1];
        }
    }

    /// <summary>The highest window id handed out.</summary>
    public int MaxId
    {
        get
        {
            lock (_gate)
            {
                return _byId.Count;
            }
        }
    }

    /// <summary>The open windows no placed buffer pins and none is releasing: what <see cref="WindowResidency"/> may release.</summary>
    public List<SectionWindow> Releasable()
    {
        lock (_gate)
        {
            return [.. _all.Values.Where(window => window.Heap != 0 && !window.Pinned && !window.Retiring)];
        }
    }

    /// <summary>
    /// Opens the closed window <paramref name="window"/> for the address map if it fits; false when it does not or the
    /// desktop refuses it. The map hears of it through <see cref="WindowOpened"/>.
    /// </summary>
    public bool TryOpen(nint device, SectionWindow window)
    {
        nint heap = Heap(device, window.Chunk, window.Start, SectionHeapUse.Asked, out _, out _);

        return heap != 0;
    }

    /// <summary>The room the address map has now (<see cref="Room"/>).</summary>
    public ulong MapRoom()
    {
        lock (_gate)
        {
            _sinceRead = ReadInterval;

            return Room(SectionHeapUse.Map);
        }
    }

    /// <summary>Marks <paramref name="window"/> released at a present: no request opens it until <see cref="Close"/>.</summary>
    public void Retire(SectionWindow window)
    {
        lock (_gate)
        {
            window.Retiring = true;
        }
    }

    /// <summary>Releases the heap of a retired window whose buffers are gone; the view stays for a reopen.</summary>
    public void Close(SectionWindow window)
    {
        lock (_gate)
        {
            if (window.Heap != 0)
            {
                ComObject.Release(window.Heap);
                window.Heap = 0;
                _opened -= Math.Min(_opened, window.Bytes);
            }

            window.Retiring = false;
            _sinceRead = ReadInterval;
        }
    }

    private SectionWindow Find(XMemPageChunk chunk, ulong start, ulong bytes)
    {
        if (_all.TryGetValue((chunk.Section, start), out var window))
        {
            return window;
        }

        int id = 0;

        if (_byId.Count < Binding.WindowFeedback.MaxIds)
        {
            id = _byId.Count + 1;
        }

        window = new SectionWindow(chunk, start, bytes, id);
        _all[(chunk.Section, start)] = window;

        if (id != 0)
        {
            _byId.Add(window);
        }

        return window;
    }

    /// <summary>The section's window size, chosen at its first request: whole windows when all of it fits, lazy ones otherwise.</summary>
    private ulong Window(nint section, ulong sectionBytes)
    {
        if (_windows.TryGetValue(section, out ulong window))
        {
            return window;
        }

        window = sectionBytes;
        ulong room = Room(SectionHeapUse.Map);

        if (sectionBytes > InPlaceWindow && InPlaceWindow < WindowBytes)
        {
            window = InPlaceWindow;
        }
        else if (sectionBytes > WindowBytes && sectionBytes <= room)
        {
            window = WindowBytes;
        }
        else if (sectionBytes > WindowBytes)
        {
            window = LazyWindowBytes;
            _log.Warn($"[d3d12_xs] section 0x{section:x} ({sectionBytes / Mebibyte} MiB) does not fit the {room / Mebibyte} MiB of section-heap room ({Describe()}): backed in {window / Mebibyte} MiB windows as they are used, pages without one read as zeros");
        }

        _windows[section] = window;

        return window;
    }

    /// <summary>The bytes a request may open: the cap and the system-memory budget less what is open, used and kept.</summary>
    private ulong Room(SectionHeapUse use)
    {
        ulong room = Math.Min(Cap - Math.Min(Cap, _opened), SystemRoom());

        if (use != SectionHeapUse.Placed)
        {
            room -= Math.Min(room, PlacedReserve);
        }

        return room;
    }

    /// <summary>The system-memory budget less the process's use and <see cref="Headroom"/>, read every <see cref="ReadInterval"/> checks.</summary>
    private ulong SystemRoom()
    {
        if (Budget is null || ++_sinceRead <= ReadInterval)
        {
            return _systemRoom;
        }

        _sinceRead = 0;
        _systemRoom = ulong.MaxValue;

        if (Budget.TryReadSystem(out ulong budget, out ulong usage))
        {
            ulong used = usage + Headroom;
            _systemRoom = budget - Math.Min(budget, used);
        }

        return _systemRoom;
    }

    /// <summary>Logs the first window of each section and use that did not fit.</summary>
    private void Refuse(nint section, ulong start, SectionHeapUse use, ulong room)
    {
        if (_refusals.Add((section, use)))
        {
            _log.Warn($"[d3d12_xs] section 0x{section:x}: no heap at +0x{start:x} for {Name(use)}, {room / Mebibyte} MiB of room left ({Describe()}); later windows of it wait the same way");
        }
    }

    /// <summary>Opens the window's heap (its view first, if it has none); 0 when the desktop refuses it, never asked again.</summary>
    private nint OpenWindow(nint device, SectionWindow window, ulong sectionBytes, SectionHeapUse use)
    {
        var chunk = window.Chunk;

        if (window.View == 0)
        {
            window.View = chunk.Window;

            if (window.Bytes != sectionBytes)
            {
                window.View = KernelBase.MapViewOfFile3(chunk.Section, Kernel32.GetCurrentProcess(), 0, window.Start, (nuint)window.Bytes, 0, Win32Constants.PageReadWrite, null, 0);
            }
        }

        nint heap = 0;

        if (window.View != 0)
        {
            heap = _open(device, window.View);
        }

        if (_log.DetailEnabled)
        {
            string result = heap == 0 ? "failed" : $"{(_opened + window.Bytes) / Mebibyte} MiB of section heaps open";
            _log.Detail($"  [d3d12_xs] heap over section 0x{chunk.Section:x} at +0x{window.Start:x} (0x{window.Bytes:x} bytes at 0x{window.View:x}) for {Name(use)}: {result}; {Describe()}");
        }

        if (heap == 0)
        {
            window.Failed = true;

            return 0;
        }

        window.Heap = heap;
        _opened += window.Bytes;
        _sinceRead = ReadInterval;

        return heap;
    }

    private string Describe()
    {
        string cap = Cap == ulong.MaxValue ? "no cap" : $"cap {Cap / Mebibyte} MiB";

        return $"{_opened / Mebibyte} MiB open, {cap}, GPU {Budget?.Describe() ?? "budget unknown"}";
    }

    private static string Name(SectionHeapUse use)
    {
        if (use == SectionHeapUse.Map)
        {
            return "the address map";
        }

        if (use == SectionHeapUse.Asked)
        {
            return "programs that read it";
        }

        return "a placed buffer";
    }

    private nint OpenOnDevice(nint device, nint view)
    {
        Guid device3Iid = Device3Iid;
        nint device3 = 0;
        int hr = ((delegate* unmanaged<nint, Guid*, nint*, int>)ComObject.Slot(device, ComObject.QueryInterfaceSlot))(device, &device3Iid, &device3);

        if (HResult.Failed(hr))
        {
            _log.Warn($"[d3d12_xs] QueryInterface(ID3D12Device3) failed: {HResult.Format(hr)}");

            return 0;
        }

        Guid heapIid = HeapIid;
        nint heap = 0;
        hr = ((delegate* unmanaged<nint, nint, Guid*, nint*, int>)ComObject.Slot(device3, OpenHeapFromAddressSlot))(device3, view, &heapIid, &heap);
        ComObject.Release(device3);

        if (HResult.Failed(hr))
        {
            _log.Warn($"[d3d12_xs] OpenExistingHeapFromAddress(0x{view:x}) failed: {HResult.Format(hr)}");
        }

        return heap;
    }
}
