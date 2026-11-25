// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Calls;

/// <summary>
/// The memory hooks the libHttpClient recipes install with HCMemSetFunctions: a native-heap allocator that can be
/// switched to fail, to drive the library's out-of-memory paths.
/// </summary>
internal sealed unsafe class RecipeAllocator
{
    private volatile bool _failing;
    private int _failedCalls;
    private int _live;

    public int FailedCalls => Volatile.Read(ref _failedCalls);

    /// <summary>Blocks allocated and not yet freed.</summary>
    public int Live => Volatile.Read(ref _live);

    public void SetFailing(bool failing)
    {
        _failing = failing;
    }

    public void ResetFailures()
    {
        Volatile.Write(ref _failedCalls, 0);
    }

    public nint Allocate(nuint size)
    {
        if (_failing)
        {
            return Fail();
        }

        Interlocked.Increment(ref _live);
        nuint length = size == 0 ? 1 : size;

        return (nint)NativeMemory.Alloc(length);
    }

    public void Free(nint block)
    {
        if (block == 0)
        {
            return;
        }

        Interlocked.Decrement(ref _live);
        NativeMemory.Free((void*)block);
    }

    /// <summary>Counts a refused allocation and returns NULL.</summary>
    public nint Fail()
    {
        Interlocked.Increment(ref _failedCalls);

        return 0;
    }
}
