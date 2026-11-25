// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Diagnostics;
using System.Globalization;
using System.Text;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.D3D12X.DebugLayer;

/// <summary>
/// <c>--watch-transient</c>: checks the game's transient-memory pool every few milliseconds and logs the first
/// corrupted node with the memory around it. The pool (set up at 0x14247ac20) cuts one region into equal blocks; a
/// table of 0x58-byte block records holds each block's address (+0x10), retire token (+0x18) and a list (+0x48) of
/// 0x18-byte nodes {next, pooled object, token} that live in the block itself (0x140188430 links one per pooled
/// allocator or list; 0x144e9b480 walks the list when it takes the block again and starts a new one in the block's last
/// 0x18 bytes). A next pointer outside the pool or an object pointer outside user space is a write the game did not
/// make. The game's code is read, never changed.
/// </summary>
internal sealed unsafe class TransientPoolWatch
{
    /// <summary>Game globals: pool base, block table, block count and block size (0x14247ac2a to 0x14247ac75).</summary>
    private const ulong PoolBaseAddress = 0x1475DF1B8;
    private const ulong TableAddress = 0x1475DF1C0;
    private const ulong CountAddress = 0x1475DF1C8;
    private const ulong SizeAddress = 0x1475DF1CC;

    private const int RecordSize = 0x58;
    private const int BlockOffset = 0x10;
    private const int TokenOffset = 0x18;
    private const int ListOffset = 0x48;
    private const int NodeSize = 0x18;
    private const int NodeObjectOffset = 8;
    private const int NodeTokenOffset = 0x10;

    /// <summary>A list longer than this is taken as a loop.</summary>
    private const int MaxNodes = 1 << 16;

    private const ulong LowestUserAddress = 0x10000;
    private const ulong HighestUserAddress = 0x7FFF_FFFF_FFFF;
    private const int PollMilliseconds = 5;
    private const int DumpBytes = 0x60;
    private const int DumpRow = 16;
    private const int HighShift = 32;

    /// <summary>High dwords (4 GiB each) either side of the block records that the store trace watches.</summary>
    private const uint HeapSpanHighDwords = 4;
    private const int HeapSpanSlot = 2;
    private const ulong PointerAlignmentMask = 7;

    /// <summary>Pooled objects are 0x30-byte entries of arrays that may start 4 bytes into an 8-byte word.</summary>
    private const ulong ObjectAlignmentMask = 3;

    /// <summary>A node must look corrupted on this many polls in a row (the game may be relinking it).</summary>
    private const int Confirmations = 3;

    /// <summary>Root descriptor addresses inside the pool that the watch keeps (a ring).</summary>
    private const int RootRing = 4096;

    /// <summary>Root addresses printed per report.</summary>
    private const int RootsPrinted = 24;

    private static readonly RootAddress[] Roots = new RootAddress[RootRing];
    private static ulong _poolStart;
    private static ulong _poolEnd;
    private static long _rootCount;

    /// <summary>Operations of the window listed per report, and user-data ranges listed per list.</summary>
    private const int WindowOperations = 400;
    private const int RangesShown = 32;

    private readonly LoaderLog _log;
    private readonly Stopwatch _clock = Stopwatch.StartNew();
    private ulong _suspect;
    private int _seen;

    /// <summary>
    /// Breadcrumb markers (with <c>--d3d12-breadcrumbs</c>): taken before each check, kept from the last clean one, and
    /// taken after the check that first saw the corrupted node; the GPU work between them wrote it, or was writing it.
    /// </summary>
    private uint[] _markers = [];
    private uint[] _clean = [];
    private uint[] _first = [];
    private double _cleanAt;
    private double _firstAt;

    private TransientPoolWatch(LoaderLog log)
    {
        _log = log;
    }

    /// <summary>Starts the watch thread; it waits for the game to set the pool up.</summary>
    public static void Start(LoaderLog log)
    {
        TransientPoolViews.Enable();
        var watch = new TransientPoolWatch(log);
        var thread = new Thread(watch.Run) { IsBackground = true, Name = "fivestar transient watch" };
        thread.Start();
    }

    private void Run()
    {
        bool announced = false;

        while (true)
        {
            Thread.Sleep(PollMilliseconds);
            ulong pool = Read(PoolBaseAddress);
            ulong table = Read(TableAddress);
            uint count = *(uint*)CountAddress;
            uint size = *(uint*)SizeAddress;

            if (pool == 0 || table == 0 || count == 0 || size == 0)
            {
                continue;
            }

            if (!announced)
            {
                _log.Info($"  [transient] watching the pool at 0x{pool:x}: {count} blocks of 0x{size:x} bytes, records at 0x{table:x}");
                announced = true;
                _poolStart = pool;
                Volatile.Write(ref _poolEnd, pool + ((ulong)count * size));
                TransientPoolHits.Pool(pool, pool + ((ulong)count * size));

                if (GpuBreadcrumbs.Active)
                {
                    TransientPoolLifetime.Setup(pool, count, size, _log);
                }

                TraceRanges(pool, (ulong)count * size, table);
            }

            TakeMarkers();
            PoolStoreTrace.LogOutside(_log);

            if (Check(pool, table, count, size))
            {
                return;
            }
        }
    }

    /// <summary>
    /// A root CBV, SRV or UAV address a list set (slots 41 to 46): kept when it lies in the pool, with the list's pipeline
    /// at the time, so a report can name the GPU work that read or wrote the corrupted block.
    /// </summary>
    [HotPath]
    public static void Note(ulong address, nint pipeline, bool compute)
    {
        if (address < _poolStart || address >= Volatile.Read(ref _poolEnd))
        {
            return;
        }

        long index = Interlocked.Increment(ref _rootCount) - 1;
        Roots[index % RootRing] = new RootAddress(address, pipeline, compute, Stopwatch.GetTimestamp());
    }

    /// <summary>True once a corruption was confirmed and logged.</summary>
    private bool Check(ulong pool, ulong table, uint count, uint size)
    {
        ulong end = pool + ((ulong)count * size);

        for (uint i = 0; i < count; i++)
        {
            byte* record = (byte*)(table + ((ulong)i * RecordSize));
            TransientPoolLifetime.Check(i, record);
            ulong node = *(ulong*)(record + ListOffset);
            ulong previous = (ulong)(record + ListOffset);

            for (int n = 0; node != 0 && n < MaxNodes; n++)
            {
                if (!Valid(node, pool, end))
                {
                    return Suspect(i, record, previous, node, pool, size);
                }

                previous = node;
                node = *(ulong*)node;
            }
        }

        _seen = 0;
        (_clean, _markers) = (_markers, _clean);
        _cleanAt = _clock.Elapsed.TotalMilliseconds;

        return false;
    }

    /// <summary>The breadcrumb markers before a check, when the breadcrumbs are on.</summary>
    private void TakeMarkers()
    {
        if (!GpuBreadcrumbs.Active)
        {
            return;
        }

        if (_markers.Length == 0)
        {
            _markers = new uint[GpuBreadcrumbs.Slots];
            _clean = new uint[GpuBreadcrumbs.Slots];
            _first = new uint[GpuBreadcrumbs.Slots];
        }

        GpuBreadcrumbs.Snapshot(_markers);
    }

    /// <summary>A node address inside the pool whose pooled-object pointer is a user-space pointer.</summary>
    private static bool Valid(ulong node, ulong pool, ulong end)
    {
        if (node < pool || node + NodeSize > end || (node & PointerAlignmentMask) != 0)
        {
            return false;
        }

        ulong item = *(ulong*)(node + NodeObjectOffset);

        return item == 0 || (item >= LowestUserAddress && item <= HighestUserAddress && (item & ObjectAlignmentMask) == 0);
    }

    private bool Suspect(uint index, byte* record, ulong holder, ulong node, ulong pool, uint size)
    {
        if (node != _suspect)
        {
            _suspect = node;
            _seen = 0;
        }

        _seen++;

        if (_seen == 1 && _first.Length != 0)
        {
            GpuBreadcrumbs.Snapshot(_first);
            _firstAt = _clock.Elapsed.TotalMilliseconds;
        }

        if (_seen < Confirmations)
        {
            return false;
        }

        ulong block = *(ulong*)(record + BlockOffset);
        ulong token = *(ulong*)(record + TokenOffset);

        var text = new StringBuilder();
        text.Append($"[transient] corrupted node list at {_clock.Elapsed.TotalSeconds:F1} s: block {index} (0x{block:x}, record 0x{(ulong)record:x}, token 0x{token:x}) ");
        text.Append($"reaches 0x{node:x} through 0x{holder:x} (a block's last node is at offset 0x{size - NodeSize:x})");
        ulong end = pool + ((ulong)*(uint*)CountAddress * size);
        ulong shown = node >= pool && node + NodeSize <= end ? node : holder;

        if (shown != (ulong)(record + ListOffset))
        {
            text.Append($"; node at 0x{shown:x} (pool offset 0x{shown - pool:x}): next 0x{*(ulong*)shown:x}, object 0x{*(ulong*)(shown + NodeObjectOffset):x}, token 0x{*(uint*)(shown + NodeTokenOffset):x}");
            Dump(text, shown, pool, end);
        }

        Recent(text, block, size);
        Writers(text, block, size);
        _log.Error(text.ToString());
        QueueProgress.Report("transient pool corrupted");

        return true;
    }

    /// <summary>The pool bytes around <paramref name="at"/>, 8-byte words per row.</summary>
    private static void Dump(StringBuilder text, ulong at, ulong pool, ulong end)
    {
        ulong first = Math.Max(pool, (at - DumpBytes) & ~(ulong)(DumpRow - 1));
        ulong last = Math.Min(end, at + DumpBytes);

        for (ulong row = first; row < last; row += DumpRow)
        {
            text.Append($"\n    0x{row:x}: {*(ulong*)row:x16} {*(ulong*)(row + sizeof(ulong)):x16}");
        }
    }

    /// <summary>The newest root addresses that fell inside the block at <paramref name="block"/>.</summary>
    private static void Recent(StringBuilder text, ulong block, uint size)
    {
        long count = Interlocked.Read(ref _rootCount);
        int printed = 0;
        text.Append($"\n  root addresses in the pool so far: {count}; in this block, newest first:");

        for (long i = count - 1; i >= 0 && i >= count - RootRing && printed < RootsPrinted; i--)
        {
            var root = Roots[i % RootRing];

            if (root.Address >= block && root.Address < block + size)
            {
                double age = Stopwatch.GetElapsedTime(root.At).TotalMilliseconds;
                string kind = root.Compute ? "compute" : "graphics";
                text.Append($"\n    +0x{root.Address - block:x} {kind} {age:F0} ms ago, {QueueProgress.PipelineName(root.Pipeline)}");
                printed++;
            }
        }
    }

    /// <summary>
    /// With breadcrumbs: the GPU operations between the last clean check and the first corrupt one, the user-data
    /// ranges of those operations that reach the block, and the newest ranges of any operation that reach it.
    /// </summary>
    private void Writers(StringBuilder text, ulong block, uint size)
    {
        if (_first.Length == 0)
        {
            return;
        }

        var operations = new HashSet<(uint, int)>();
        text.Append(CultureInfo.InvariantCulture, $"\n  GPU operations between the last clean check and the first corrupt one ({_firstAt - _cleanAt:F1} ms):");
        int listed = GpuBreadcrumbs.DescribeWindow(text, _clean, _first, operations, WindowOperations, QueueProgress.PipelineName);

        if (listed == 0)
        {
            text.Append("\n    none");
        }

        text.Append("\n  their user-data ranges that reach this block:");
        TransientPoolHits.Describe(text, block, size, operations, RangesShown, QueueProgress.PipelineName);
        text.Append("\n  newest user-data ranges of any operation that reach this block:");
        TransientPoolHits.Describe(text, block, size, null, RangesShown, QueueProgress.PipelineName);
        PoolStoreTrace.Describe(text, block, size, RangesShown);
    }

    /// <summary>
    /// --trace-pool-stores: traces stores into the pool and into the game heap allocation the block records live in
    /// (where the fixed-size pool pages and their descriptors are too).
    /// </summary>
    private static void TraceRanges(ulong pool, ulong bytes, ulong table)
    {
        if (!PoolStoreTrace.Enabled)
        {
            return;
        }

        PoolStoreTrace.SetRange(0, pool, bytes);
        TransientPoolViews.PoolKnown();
        var (start, length) = Allocation(table);

        if (length != 0)
        {
            PoolStoreTrace.SetRange(1, start, length);
        }

        // Every global store within 16 GiB of the block records: the game heap, its fixed-size pool pages and their
        // descriptors (boot 13's corrupted pool object lay 7 GiB from the pool).
        uint high = (uint)(table >> HighShift);
        PoolStoreTrace.SetSpan(HeapSpanSlot, high - HeapSpanHighDwords, high + HeapSpanHighDwords);
    }

    /// <summary>The whole allocation (every region with the same allocation base) that holds <paramref name="address"/>.</summary>
    private static (ulong Start, ulong Bytes) Allocation(ulong address)
    {
        MemoryBasicInformation info;
        nint process = Kernel32.GetCurrentProcess();

        if (Kernel32.VirtualQueryEx(process, (nint)address, &info, (nuint)sizeof(MemoryBasicInformation)) == 0)
        {
            return (0, 0);
        }

        nint allocation = info.AllocationBase;
        ulong next = (ulong)allocation;

        while (Kernel32.VirtualQueryEx(process, (nint)next, &info, (nuint)sizeof(MemoryBasicInformation)) != 0 && info.AllocationBase == allocation)
        {
            next = (ulong)info.BaseAddress + (ulong)info.RegionSize;
        }

        return ((ulong)allocation, next - (ulong)allocation);
    }

    private static ulong Read(ulong address)
    {
        return *(ulong*)address;
    }
}
