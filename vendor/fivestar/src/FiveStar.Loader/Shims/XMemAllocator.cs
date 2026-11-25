// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Collections.Concurrent;
using System.Runtime.InteropServices;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Loading;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims;

/// <summary>
/// The xmem.dll title allocator on VirtualAlloc and the aligned native heap. XALLOC_ATTRIBUTES is a 64-bit bitfield
/// (GDK xmem.h): bits 0..13 ObjectType, 14..15 PageSize, 16..23 AllocatorId, 24..28 Alignment, 29..32 MemoryType.
/// </summary>
internal sealed unsafe class XMemAllocator
{
    /// <summary>Requests at or above this size go straight to VirtualAlloc (page granular, zeroed).</summary>
    private const ulong LargeAllocationThreshold = 64 * 1024;

    // XALLOC_ATTRIBUTES fields (xmem.h).
    private const int PageSizeShift = 14;
    private const ulong PageSizeMask = 0x3;
    private const int AlignmentShift = 24;
    private const ulong AlignmentMask = 0x1F;

    /// <summary>Alignment code n means 1 &lt;&lt; (n - 1), capped here; code 0 is the default.</summary>
    private const int MaxAlignmentLog2 = 20;

    /// <summary>Never below the x64 malloc guarantee.</summary>
    private const ulong MinimumAlignment = 16;

    /// <summary>XMemVirtualQuery fills a 0x40-byte block: MEMORY_BASIC_INFORMATION, then an Xbox-only tail.</summary>
    private const int XboxTailOffset = 0x30;
    private const int XboxTailSize = 0x10;

    private readonly ConcurrentDictionary<nint, XMemAllocation> _allocations = new();
    private readonly ImageTable _images;
    private readonly LoaderLog _log;
    private long _liveBytes;
    private long _allocationCount;
    private long _freeCount;
    private volatile bool _traceNextAllocation;

    public XMemAllocator(ImageTable images, LoaderLog log)
    {
        _images = images;
        _log = log;
    }

    public long LiveBytes => Interlocked.Read(ref _liveBytes);

    public long AllocationCount => Interlocked.Read(ref _allocationCount);

    public long FreeCount => Interlocked.Read(ref _freeCount);

    /// <summary>Whether <paramref name="block"/> is a live XMemAlloc block of its own VirtualAlloc (the memory map).</summary>
    public bool IsVirtualBlock(nint block)
    {
        return _allocations.TryGetValue(block, out var allocation) && allocation.IsVirtual;
    }

    /// <summary>Logs a native stack walk from inside the next XMemAlloc (shows unwinding through mapped images).</summary>
    public void TraceNextAllocation()
    {
        _traceNextAllocation = true;
    }

    public static nuint AlignmentFromAttributes(ulong attributes)
    {
        int code = (int)((attributes >> AlignmentShift) & AlignmentMask);
        ulong alignment = MinimumAlignment;

        if (code != 0)
        {
            alignment = 1UL << Math.Min(code - 1, MaxAlignmentLog2);
        }

        return (nuint)Math.Max(alignment, MinimumAlignment);
    }

    [HotPath]
    public nint Allocate(nuint size, ulong attributes)
    {
        if (_traceNextAllocation)
        {
            TraceAllocation();
        }

        bool isVirtual = size >= LargeAllocationThreshold || ((attributes >> PageSizeShift) & PageSizeMask) != 0;
        nint block = AllocateBlock(size, attributes, isVirtual);
        Interlocked.Increment(ref _allocationCount);

        if (block != 0)
        {
            _allocations[block] = new XMemAllocation(size, isVirtual);
            Interlocked.Add(ref _liveBytes, (long)size);
        }

        if (_log.DetailEnabled)
        {
            LogAllocation(size, attributes, block);
        }

        return block;
    }

    [HotPath]
    public void Free(nint block, ulong attributes)
    {
        if (_log.DetailEnabled)
        {
            LogFree(block, attributes);
        }

        if (block == 0)
        {
            return;
        }

        Interlocked.Increment(ref _freeCount);

        if (!_allocations.TryRemove(block, out var allocation))
        {
            WarnUnknownFree(block);

            return;
        }

        Interlocked.Add(ref _liveBytes, -(long)allocation.Size);

        if (allocation.IsVirtual)
        {
            _ = Kernel32.VirtualFree(block, 0, Win32Constants.MemRelease);

            return;
        }

        NativeMemory.AlignedFree((void*)block);
    }

    /// <summary>
    /// XMemVirtualQuery: VirtualQueryEx into the first 0x30 bytes. The tail (+0x30..+0x3F) is zero; its meaning is
    /// unknown (the BVH builder's validation tests bits 0..1 of +0x30 and only logs when they are clear).
    /// </summary>
    public int VirtualQuery(nint process, nint address, byte* info)
    {
        var basic = (MemoryBasicInformation*)info;
        nint target = process;

        if (target == 0)
        {
            target = Kernel32.GetCurrentProcess();
        }

        nuint written = Kernel32.VirtualQueryEx(target, address, basic, (nuint)sizeof(MemoryBasicInformation));
        new Span<byte>(info + XboxTailOffset, XboxTailSize).Clear();

        if (_log.DetailEnabled)
        {
            _log.Detail($"  [xmem] XMemVirtualQuery(0x{address:x}) -> {written} state=0x{basic->State:x} protect=0x{basic->Protect:x}");
        }

        int found = written != 0 ? 1 : 0;

        return found;
    }

    private void TraceAllocation()
    {
        _traceNextAllocation = false;
        NativeStack.Log(_images, _log, "XMemAlloc call stack (RtlLookupFunctionEntry/RtlVirtualUnwind walk)");
    }

    private void LogAllocation(nuint size, ulong attributes, nint block)
    {
        _log.Detail($"  [xmem] XMemAlloc(size=0x{size:x}, attr=0x{attributes:x}) -> 0x{block:x}");
    }

    private void LogFree(nint block, ulong attributes)
    {
        _log.Detail($"  [xmem] XMemFree(0x{block:x}, attr=0x{attributes:x})");
    }

    private void WarnUnknownFree(nint block)
    {
        _log.Warn($"[xmem] XMemFree(0x{block:x}): pointer not allocated by XMemAlloc - ignored");
    }

    private static nint AllocateBlock(nuint size, ulong attributes, bool isVirtual)
    {
        if (isVirtual)
        {
            return Kernel32.VirtualAlloc(0, size, Win32Constants.MemReserveCommit, Win32Constants.PageReadWrite);
        }

        nuint length = size == 0 ? 1 : size;

        return (nint)NativeMemory.AlignedAlloc(length, AlignmentFromAttributes(attributes));
    }
}
