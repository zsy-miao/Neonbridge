// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;
using FiveStar.Loader.Shims.D3D12X.Binding;
using FiveStar.Loader.Shims.D3D12X.Present;

namespace FiveStar.Loader.Shims.D3D12X;

/// <summary>
/// <c>--evict-windows</c> (prototype, docs/physical-memory.md#gpu-visibility): the section windows of the address map
/// are a cache over the GPU's system-memory budget, not all resident at once. Once per present, before the first list
/// of the frame runs (<see cref="Poll"/>): the windows programs asked for (<see cref="WindowFeedback"/>: a read on a
/// waiting page sets its window's flag) open while they fit; when they do not, the open windows wanted least recently
/// are released for them. A released window's pages wait again at once, reading zeros; its views and heap go once every
/// queue has finished what was submitted until then (each queue's shim fence), and no sooner than
/// <see cref="RetirePresents"/> presents. Windows a placed buffer of the game lives on are never released.
/// </summary>
internal sealed unsafe class WindowResidency
{
    /// <summary>Presents between releasing a window and releasing its heap: past the GPU work that could still read it.</summary>
    public const long RetirePresents = 8;

    /// <summary>Windows opened per present at most, so one frame does not wait on many heaps.</summary>
    public const int OpensPerPresent = 8;

    /// <summary>Presents a window keeps after it opened or was asked for, before it may be released for another.</summary>
    public const long KeepPresents = 30;

    private readonly object _gate = new();
    private readonly System.Collections.Concurrent.ConcurrentQueue<int> _asked = new();
    private readonly List<(SectionWindow Window, List<nint> Buffers, long Present, (nint Fence, ulong Value)[] Marks)> _retiring = [];
    private readonly SectionHeaps _heaps;
    private readonly GameMemoryViews _views;
    private readonly GpuFrameThrottle _throttle;
    private readonly LoaderLog _log;
    private long _present = -1;
    private long _opened;
    private long _released;
    private long _wanted;

    public WindowResidency(SectionHeaps heaps, GameMemoryViews views, GpuFrameThrottle throttle, LoaderLog log)
    {
        _heaps = heaps;
        _views = views;
        _throttle = throttle;
        _log = log;
    }

    /// <summary>Windows opened for programs that asked for them, released for others, and asks seen.</summary>
    public string Summary => $"section windows: {Interlocked.Read(ref _wanted)} asked for, {Interlocked.Read(ref _opened)} opened for them, {Interlocked.Read(ref _released)} released";

    /// <summary>
    /// The CPU asks for window <paramref name="id"/>: a view is made over a page that waits for it
    /// (<see cref="GameMemoryViews.Maps"/>). Taken at the next <see cref="Poll"/> as a program's flag would be.
    /// </summary>
    public void Ask(int id)
    {
        _asked.Enqueue(id);
    }

    /// <summary>
    /// Once per <paramref name="present"/>: releases the heaps retired long enough ago, then opens the windows whose
    /// flags programs set in <paramref name="flags"/> (the descriptor window's <see cref="WindowFeedback.Base"/>),
    /// releasing others for them when they do not fit.
    /// </summary>
    public void Poll(nint device, uint* flags, long present)
    {
        if (flags == null || Volatile.Read(ref _present) == present)
        {
            return;
        }

        lock (_gate)
        {
            if (_present == present)
            {
                return;
            }

            _present = present;
            Retire(present);
            Open(device, Wanted(flags, present), present);
        }
    }

    /// <summary>The closed windows whose flags are set, oldest id first; the flags are cleared.</summary>
    private List<SectionWindow> Wanted(uint* flags, long present)
    {
        var wanted = new List<SectionWindow>();
        int last = _heaps.MaxId;

        while (_asked.TryDequeue(out int asked))
        {
            if (asked > 0 && asked <= last)
            {
                flags[asked] = 1;
            }
        }

        for (int id = 1; id <= last; id++)
        {
            if (Volatile.Read(ref flags[id]) == 0)
            {
                continue;
            }

            flags[id] = 0;
            var window = _heaps.ById(id);

            if (window is null)
            {
                continue;
            }

            window.LastWanted = present;
            Interlocked.Increment(ref _wanted);

            if (window.Heap == 0 && !window.Retiring && !window.Failed)
            {
                wanted.Add(window);
            }
        }

        return wanted;
    }

    private void Open(nint device, List<SectionWindow> wanted, long present)
    {
        int opened = 0;

        foreach (var window in wanted)
        {
            if (opened == OpensPerPresent)
            {
                return;
            }

            if (_heaps.MapRoom() < window.Bytes)
            {
                MakeRoom(window.Bytes, present);

                continue;
            }

            if (_heaps.TryOpen(device, window))
            {
                window.LastWanted = present;
                opened++;
                Interlocked.Increment(ref _opened);
            }
        }
    }

    /// <summary>Releases the windows wanted least recently until <paramref name="bytes"/> more will be free once they retire.</summary>
    private void MakeRoom(ulong bytes, long present)
    {
        ulong pending = 0;

        foreach (var retiring in _retiring)
        {
            pending += retiring.Window.Bytes;
        }

        if (pending >= bytes)
        {
            return;
        }

        foreach (var window in _heaps.Releasable().Where(window => present - window.LastWanted > KeepPresents).OrderBy(window => window.LastWanted))
        {
            _heaps.Retire(window);
            _retiring.Add((window, _views.Close(window), present, _throttle.Submitted()));
            Interlocked.Increment(ref _released);
            pending += window.Bytes;

            if (pending >= bytes)
            {
                return;
            }
        }
    }

    /// <summary>
    /// Releases the view buffers and heaps of the windows retired <see cref="RetirePresents"/> presents ago or more whose
    /// queues have finished everything submitted when they were retired: a list in flight may read any window.
    /// </summary>
    private void Retire(long present)
    {
        for (int i = _retiring.Count - 1; i >= 0; i--)
        {
            var (window, buffers, released, marks) = _retiring[i];

            if (present - released < RetirePresents || !GpuFrameThrottle.Passed(marks))
            {
                continue;
            }

            foreach (nint buffer in buffers)
            {
                ComObject.Release(buffer);
            }

            _heaps.Close(window);
            _retiring.RemoveAt(i);

            if (_log.DetailEnabled)
            {
                _log.Detail($"  [d3d12_xs] section 0x{window.Chunk.Section:x} window at +0x{window.Start:x} released; {Summary}");
            }
        }
    }
}
