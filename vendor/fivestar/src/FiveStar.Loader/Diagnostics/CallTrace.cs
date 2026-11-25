// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Codegen;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Diagnostics;

/// <summary>
/// Records the most recent calls through wrapped imports, for fault reports. Each wrapped import gets a thunk that
/// adds its id to a native ring with <c>lock xadd</c> and jumps to the target, using only r10 and r11 (volatile, never
/// arguments in the Windows x64 calling convention), so arguments and the stack reach the target unchanged. The ring
/// lives as long as the process, because mapped threads may still call through the thunks.
/// </summary>
internal sealed unsafe class CallTrace
{
    /// <summary>Ring entries; a power of two, so the slot is the counter masked.</summary>
    private const int Capacity = 256;

    /// <summary>The ring starts with the 64-bit call counter; the 32-bit ids follow it.</summary>
    private const byte IdsOffset = sizeof(ulong);

    /// <summary>Wrapped imports with a call counter of their own; later ones are recorded in the ring only.</summary>
    private const int CountedIds = 65536;

    private readonly object _gate = new();
    private readonly List<string> _names = [];
    private readonly CodeHeap _heap;
    private readonly nint _ring;
    private readonly nint _counts;

    public CallTrace(CodeHeap heap)
    {
        _heap = heap;
        _ring = Kernel32.VirtualAlloc(0, Win32Constants.PageSize, Win32Constants.MemReserveCommit, Win32Constants.PageReadWrite);

        if (_ring == 0)
        {
            throw new InvalidOperationException("CallTrace VirtualAlloc failed");
        }

        _counts = Kernel32.VirtualAlloc(0, CountedIds * sizeof(ulong), Win32Constants.MemReserveCommit, Win32Constants.PageReadWrite);

        if (_counts == 0)
        {
            throw new InvalidOperationException("CallTrace VirtualAlloc failed");
        }
    }

    /// <summary>Whether imports are wrapped; off until <see cref="Enable"/>.</summary>
    public bool Enabled { get; private set; }

    /// <summary>Calls recorded so far.</summary>
    public ulong CallCount => Volatile.Read(ref *(ulong*)_ring);

    /// <summary>Whether imports bound to System32 are wrapped too (many more calls, much shorter history).</summary>
    public bool IncludesSystem { get; private set; }

    /// <summary>Mapped images whose exports are wrapped too (<c>--trace-module</c>), lower case.</summary>
    public HashSet<string> TracedModules { get; } = new(StringComparer.OrdinalIgnoreCase);

    public void Enable(bool includeSystem)
    {
        Enabled = true;
        IncludesSystem = includeSystem;
    }

    /// <summary>A thunk that records <paramref name="name"/> and jumps to <paramref name="target"/>.</summary>
    public nint Wrap(nint target, string name)
    {
        uint id;

        lock (_gate)
        {
            id = (uint)_names.Count;
            _names.Add(name);
        }

        var code = new X64Emitter();
        code.MovR10((ulong)_ring);
        code.MovR11d(1);
        code.LockXaddR10R11();
        code.AndR11d(Capacity - 1);
        code.StoreDwordR10IndexR11(IdsOffset, id);

        if (id < CountedIds)
        {
            code.MovR10((ulong)(_counts + ((nint)id * sizeof(ulong))));
            code.LockIncR10();
        }

        code.MovR10((ulong)target);
        code.JmpR10();

        return _heap.Write(code.ToArray());
    }

    /// <summary>The calls through each wrapped name so far (several imports of one name are added up).</summary>
    public Dictionary<string, ulong> Totals()
    {
        var totals = new Dictionary<string, ulong>(StringComparer.Ordinal);
        ulong* counts = (ulong*)_counts;

        lock (_gate)
        {
            for (int id = 0; id < Math.Min(_names.Count, CountedIds); id++)
            {
                ulong calls = Volatile.Read(ref counts[id]);

                if (calls != 0)
                {
                    totals[_names[id]] = totals.GetValueOrDefault(_names[id]) + calls;
                }
            }
        }

        return totals;
    }

    /// <summary>The names of the last <paramref name="count"/> recorded calls, oldest first.</summary>
    public IReadOnlyList<string> Recent(int count)
    {
        ulong total = CallCount;
        int taken = (int)Math.Min((ulong)Math.Min(count, Capacity), total);
        var recent = new List<string>(taken);
        uint* ids = (uint*)(_ring + IdsOffset);

        lock (_gate)
        {
            for (ulong call = total - (ulong)taken; call < total; call++)
            {
                int id = (int)ids[call & (Capacity - 1)];
                recent.Add(_names[id]);
            }
        }

        return recent;
    }
}
