// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Apu;

/// <summary>
/// The audio processor (APU) heap of acphal.dll on ordinary memory. On the console the APU shares memory with the CPU
/// and names it by a 32-bit APU address; here the heap is one VirtualAlloc region and an APU address is the offset
/// into it plus <see cref="ApuAddressBase"/>, so translation both ways is exact and 0 is never a valid address. No
/// audio hardware reads it, which is fine: the SHAPE and XDsp calls that would hand it to hardware are not available.
/// </summary>
internal sealed class ApuHeap : IDisposable
{
    /// <summary>The APU address of the first heap byte. Arbitrary (the real values are hardware addresses), only nonzero.</summary>
    public const uint ApuAddressBase = 0x10000;

    /// <summary>Alignment when the caller asks for none; the x64 malloc guarantee.</summary>
    private const ulong DefaultAlignment = 16;

    private readonly object _gate = new();
    private readonly LoaderLog _log;
    private RangeAllocator? _allocator;
    private nint _base;

    public ApuHeap(LoaderLog log)
    {
        _log = log;
    }

    /// <summary>ApuCreateHeap: one region for the cached and the non-cached part together.</summary>
    public int Create(uint cachedBytes, uint nonCachedBytes)
    {
        ulong size = AlignToPage((ulong)cachedBytes + nonCachedBytes);

        lock (_gate)
        {
            if (_allocator is not null)
            {
                return HResult.IllegalMethodCall;
            }

            if (size == 0 || size > uint.MaxValue - ApuAddressBase)
            {
                return HResult.InvalidArgument;
            }

            _base = Kernel32.VirtualAlloc(0, (nuint)size, Win32Constants.MemReserveCommit, Win32Constants.PageReadWrite);

            if (_base == 0)
            {
                return HResult.OutOfMemory;
            }

            _allocator = new RangeAllocator(size);
        }

        _log.Info($"  [acphal] ApuCreateHeap(cached=0x{cachedBytes:x}, non-cached=0x{nonCachedBytes:x}) -> 0x{_base:x} (ordinary memory)");

        return HResult.Ok;
    }

    /// <summary>ApuAlloc: a zeroed block of the heap, its virtual address and (optionally) its APU address.</summary>
    public unsafe int Allocate(nint* virtualAddress, uint* apuAddress, uint size, uint alignment)
    {
        if (virtualAddress == null)
        {
            return HResult.Pointer;
        }

        ulong? offset;

        lock (_gate)
        {
            if (_allocator is null)
            {
                return HResult.IllegalMethodCall;
            }

            offset = _allocator.Allocate(size, Math.Max(alignment, DefaultAlignment));
        }

        if (offset is null)
        {
            _log.Warn($"[acphal] ApuAlloc(0x{size:x}): heap full");

            return HResult.OutOfMemory;
        }

        nint block = _base + (nint)offset.Value;
        NativeMemory.Clear((void*)block, size);
        *virtualAddress = block;

        if (apuAddress != null)
        {
            *apuAddress = ApuAddressBase + (uint)offset.Value;
        }

        return HResult.Ok;
    }

    /// <summary>ApuFree: returns a block; E_INVALIDARG for an address ApuAlloc did not hand out.</summary>
    public int Free(nint virtualAddress)
    {
        lock (_gate)
        {
            if (_allocator is null || !TryOffset(virtualAddress, out ulong offset) || !_allocator.Free(offset))
            {
                return HResult.InvalidArgument;
            }
        }

        return HResult.Ok;
    }

    /// <summary>ApuIsVirtualAddressValid: whether the address lies inside a live heap block.</summary>
    public bool IsValid(nint virtualAddress)
    {
        lock (_gate)
        {
            return _allocator is not null && TryOffset(virtualAddress, out ulong offset) && _allocator.Contains(offset, 1);
        }
    }

    /// <summary>ApuMapVirtualAddress: the APU address of a heap byte, 0 outside the heap.</summary>
    public uint ToApuAddress(nint virtualAddress)
    {
        lock (_gate)
        {
            if (!TryOffset(virtualAddress, out ulong offset))
            {
                return 0;
            }

            return ApuAddressBase + (uint)offset;
        }
    }

    /// <summary>ApuMapApuAddress: the virtual address of an APU address, 0 outside the heap.</summary>
    public nint ToVirtualAddress(uint apuAddress)
    {
        lock (_gate)
        {
            if (_allocator is null || apuAddress < ApuAddressBase || apuAddress - ApuAddressBase >= _allocator.Capacity)
            {
                return 0;
            }

            return _base + (nint)(apuAddress - ApuAddressBase);
        }
    }

    /// <summary>The virtual address of <paramref name="bytes"/> bytes from an APU address on, 0 unless all of them are in the heap.</summary>
    public nint ToVirtualAddress(uint apuAddress, uint bytes)
    {
        lock (_gate)
        {
            if (_allocator is null || apuAddress < ApuAddressBase || (ulong)(apuAddress - ApuAddressBase) + bytes > _allocator.Capacity)
            {
                return 0;
            }

            return _base + (nint)(apuAddress - ApuAddressBase);
        }
    }

    public void Dispose()
    {
        if (_base != 0)
        {
            _ = Kernel32.VirtualFree(_base, 0, Win32Constants.MemRelease);
            _base = 0;
        }
    }

    private static ulong AlignToPage(ulong size)
    {
        const ulong pageMask = Win32Constants.PageSize - 1;

        return (size + pageMask) & ~pageMask;
    }

    private bool TryOffset(nint virtualAddress, out ulong offset)
    {
        offset = (ulong)(virtualAddress - _base);

        return _allocator is not null && virtualAddress >= _base && offset < _allocator.Capacity;
    }
}
