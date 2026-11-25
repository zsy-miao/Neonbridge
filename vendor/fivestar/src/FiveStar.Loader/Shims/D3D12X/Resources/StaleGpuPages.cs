// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Collections.Concurrent;
using FiveStar.Loader.Diagnostics;

namespace FiveStar.Loader.Shims.D3D12X.Resources;

/// <summary>
/// Reports pages the game remaps or unmaps under a desktop buffer of the address table. Such a buffer was placed over
/// the frames mapped when it was made (<see cref="XboxPlacedResources"/>) and keeps them: on the console the GPU follows
/// the new mapping, as the address map of the translated programs does (<see cref="Binding.GameMemoryViews"/>), but
/// desktop views, copies and indirect arguments on the buffer still reach the frame of that moment. When that frame is
/// mapped again elsewhere, GPU writes through the buffer change the memory at the new address. Both are logged with
/// the addresses involved (the first <see cref="DetailedEvents"/> of each), then counted at every power of two.
/// </summary>
internal sealed class StaleGpuPages
{
    /// <summary>Events of each kind logged in full.</summary>
    private const long DetailedEvents = 16;

    /// <summary>Held frames remembered for the reuse check; past this, later ones are counted only.</summary>
    private const int MaxTrackedFrames = 1 << 16;

    private const ulong PageMask = XMemPhysicalMemory.PageSize - 1;

    /// <summary>Per buffer and GPU page whose mapping changed since the buffer was made: the frame the buffer holds (0: none, a buffer not over pages).</summary>
    private readonly ConcurrentDictionary<(nint Resource, ulong Page), ulong> _held = new();
    private readonly ConcurrentDictionary<ulong, StaleGpuPage> _frames = new();
    private readonly AddressTable _addresses;
    private readonly XMemPhysicalMemory _memory;
    private readonly LoaderLog _log;
    private long _remapped;
    private long _reused;

    public StaleGpuPages(AddressTable addresses, XMemPhysicalMemory memory, LoaderLog log)
    {
        _addresses = addresses;
        _memory = memory;
        _log = log;
    }

    /// <summary>Remaps and unmaps under a desktop buffer so far.</summary>
    public long Remapped => Interlocked.Read(ref _remapped);

    /// <summary>Times a frame such a buffer still holds was mapped at another address.</summary>
    public long Reused => Interlocked.Read(ref _reused);

    /// <summary><see cref="XMemPhysicalMemory.ObservePages"/>: the frame at <paramref name="address"/> changed.</summary>
    public void Changed(nint address, ulong previous, ulong frame)
    {
        if (Covers(address, out ulong gpu, out var range))
        {
            Moved(address, gpu, range, previous, frame);
        }

        if (frame != 0)
        {
            Mapped(address, frame);
        }
    }

    /// <summary>The page under <paramref name="range"/>'s buffer now maps <paramref name="frame"/>; the buffer keeps its own.</summary>
    private void Moved(nint address, ulong gpu, AddressRange range, ulong previous, ulong frame)
    {
        // The first change since the buffer was made: the frame before it is the one the buffer was placed over.
        var key = (range.Resource, gpu & ~PageMask);
        ulong held = _held.GetOrAdd(key, previous);

        if (held == 0)
        {
            return;
        }

        if (frame == held)
        {
            _held.TryRemove(key, out _);
            _frames.TryRemove(held, out _);

            return;
        }

        if (_frames.Count < MaxTrackedFrames)
        {
            _frames.TryAdd(held, new StaleGpuPage(gpu, address, range.GameAddress));
        }

        long count = Interlocked.Increment(ref _remapped);

        if (count <= DetailedEvents)
        {
            string change = frame == 0 ? "unmapped" : $"now 0x{frame:x}";
            _log.Warn($"[gpu pages] 0x{gpu:x} (CPU 0x{address:x}, {change}): the GPU buffer at 0x{range.GameAddress:x} (0x{range.Size:x} bytes) still reaches frame 0x{held:x} there");
        }

        Total(count);
    }

    /// <summary>A frame mapped at <paramref name="address"/>: when a buffer still holds it elsewhere, that buffer writes here.</summary>
    private void Mapped(nint address, ulong frame)
    {
        if (!_frames.TryGetValue(frame, out var stale) || stale.CpuAddress == address)
        {
            return;
        }

        long count = Interlocked.Increment(ref _reused);

        if (count <= DetailedEvents)
        {
            _log.Warn($"[gpu pages] frame 0x{frame:x} mapped again at 0x{address:x} while the GPU buffer at 0x{stale.BufferAddress:x} still holds it at 0x{stale.GpuAddress:x}: GPU writes there change the memory at 0x{address:x}");
        }

        Total(count);
    }

    /// <summary>The address table range over the page at <paramref name="address"/>, as the CPU or its GPU alias addresses it.</summary>
    private bool Covers(nint address, out ulong gpu, out AddressRange range)
    {
        gpu = (ulong)address;

        if (_addresses.TryFind(gpu, out range))
        {
            return true;
        }

        gpu = (ulong)_memory.GpuAlias(address);

        return gpu != 0 && _addresses.TryFind(gpu, out range);
    }

    private void Total(long count)
    {
        if (count <= DetailedEvents || (count & (count - 1)) != 0)
        {
            return;
        }

        _log.Info($"[gpu pages] {Remapped} remaps under GPU buffers so far; frames they still hold were mapped again elsewhere {Reused} times");
    }
}
