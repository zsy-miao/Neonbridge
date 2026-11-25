// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims;

/// <summary>
/// An address range physical pages map into (an XMemVirtualAlloc MEM_64K_PAGES reservation), kept as placeholders.
/// Each 64 KiB slot is part of a larger placeholder, its own placeholder, or a view of one page's section. Mapping
/// splits the slot out of its placeholder and replaces it with a view; unmapping leaves a placeholder behind, and
/// <see cref="Coalesce"/> merges a fully unmapped range back into one. A slot's state is changed only by the thread
/// mapping that slot, so slots need no lock; mapping one address from two threads at once is undefined, as on the console.
/// </summary>
internal sealed unsafe class XMemReservation
{
    /// <summary>Slot state: still inside a larger placeholder.</summary>
    private const ulong Unsplit = 0;

    /// <summary>Slot state: its own 64 KiB placeholder. Any other value is the frame mapped there.</summary>
    private const ulong Placeholder = ulong.MaxValue;

    private const ulong PageSize = XMemPageStore.PageSize;

    private readonly ulong[] _slots;
    private readonly nint _process;

    public XMemReservation(nint process, nint address, ulong size, uint viewProtection, nint origin)
    {
        _process = process;
        Address = address;
        Origin = origin;
        _slots = new ulong[(size + PageSize - 1) / PageSize];
        ViewProtection = viewProtection;
    }

    public nint Address { get; }

    /// <summary>The start of the game placeholder this reservation was split from, else <see cref="Address"/>.</summary>
    public nint Origin { get; }

    /// <summary>The CPU protection of every view mapped here.</summary>
    public uint ViewProtection { get; }

    public ulong Size => (ulong)_slots.Length * PageSize;

    /// <summary>
    /// For a GPU-only reservation the game never maps (protect PAGE_NOACCESS | PAGE_GRAPHICS_*), the CPU reservation
    /// of the same size made just before it: the GPU reads its pages at the same offset here (docs/gpu-resources.md).
    /// </summary>
    public XMemReservation? CpuView { get; set; }

    /// <summary>For a CPU reservation, the GPU-only reservation that aliases its pages (<see cref="CpuView"/> the other way).</summary>
    public XMemReservation? GpuAlias { get; set; }

    public bool Overlaps(nint address, ulong size)
    {
        return (ulong)address < (ulong)Address + Size && (ulong)Address < (ulong)address + size;
    }

    public bool Contains(nint address)
    {
        return (ulong)(address - Address) < Size;
    }

    /// <summary>The frame mapped at <paramref name="address"/>, or 0.</summary>
    public ulong FrameAt(nint address)
    {
        ulong state = _slots[SlotOf(address)];

        if (state == Placeholder)
        {
            return 0;
        }

        return state;
    }

    /// <summary>Maps <paramref name="frame"/> (at <paramref name="offset"/> of <paramref name="chunk"/>) over the slot holding <paramref name="address"/>.</summary>
    public bool Map(nint address, ulong frame, XMemPageChunk chunk, ulong offset)
    {
        int slot = SlotOf(address);
        nint target = SlotAddress(slot);
        ulong state = _slots[slot];

        if (state == frame)
        {
            return true;
        }

        if (state == Unsplit)
        {
            // Fails, harmlessly, when the slot is already a placeholder of its own.
            _ = Kernel32.VirtualFree(target, (nuint)PageSize, Win32Constants.MemRelease | MemoryConstants.MemPreservePlaceholder);
        }
        else if (state != Placeholder && KernelBase.UnmapViewOfFile2(_process, target, MemoryConstants.MemUnmapPreservePlaceholder) == 0)
        {
            return false;
        }

        _slots[slot] = Placeholder;
        nint view = KernelBase.MapViewOfFile3(chunk.Section, _process, target, offset, (nuint)PageSize, MemoryConstants.MemReplacePlaceholder, ViewProtection, null, 0);

        if (view == 0)
        {
            return false;
        }

        _slots[slot] = frame;

        return true;
    }

    /// <summary>Unmaps the slot holding <paramref name="address"/>, leaving a placeholder; an unmapped slot is left alone.</summary>
    public bool Unmap(nint address)
    {
        int slot = SlotOf(address);
        ulong state = _slots[slot];

        if (state == Unsplit || state == Placeholder)
        {
            return true;
        }

        if (KernelBase.UnmapViewOfFile2(_process, SlotAddress(slot), MemoryConstants.MemUnmapPreservePlaceholder) == 0)
        {
            return false;
        }

        _slots[slot] = Placeholder;

        return true;
    }

    /// <summary>Merges the split placeholders back into one when no slot is mapped.</summary>
    public void Coalesce()
    {
        foreach (ulong state in _slots)
        {
            if (state != Unsplit && state != Placeholder)
            {
                return;
            }
        }

        if (Kernel32.VirtualFree(Address, (nuint)Size, Win32Constants.MemRelease | MemoryConstants.MemCoalescePlaceholders) != 0)
        {
            Array.Clear(_slots);
        }
    }

    /// <summary>Unmaps every slot and frees the range.</summary>
    public void Release()
    {
        for (int slot = 0; slot < _slots.Length; slot++)
        {
            _ = Unmap(SlotAddress(slot));
        }

        Coalesce();
        _ = Kernel32.VirtualFree(Address, 0, Win32Constants.MemRelease);
    }

    private int SlotOf(nint address)
    {
        return (int)((ulong)(address - Address) / PageSize);
    }

    private nint SlotAddress(int slot)
    {
        return Address + (nint)((ulong)slot * PageSize);
    }
}
