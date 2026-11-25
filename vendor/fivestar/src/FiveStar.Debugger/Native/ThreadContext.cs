// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Debugger.Native;

/// <summary>A thread's CONTEXT in aligned native memory the debugger owns.</summary>
internal sealed unsafe class ThreadContext : IDisposable
{
    private Amd64Context* _context;

    private ThreadContext(Amd64Context* context)
    {
        _context = context;
    }

    public Amd64Context* Pointer => _context;

    /// <summary>The context of a thread stopped by the debug event, or null when the system refuses it.</summary>
    public static ThreadContext? Capture(nint thread)
    {
        var context = Allocate();
        context->ContextFlags = Amd64Context.All;

        if (Kernel32.GetThreadContext(thread, context) == 0)
        {
            NativeMemory.AlignedFree(context);

            return null;
        }

        return new ThreadContext(context);
    }

    /// <summary>A copy that StackWalk64 may change.</summary>
    public ThreadContext Copy()
    {
        var copy = Allocate();
        *copy = *_context;

        return new ThreadContext(copy);
    }

    public void Dispose()
    {
        NativeMemory.AlignedFree(_context);
        _context = null;
    }

    private static Amd64Context* Allocate()
    {
        var context = (Amd64Context*)NativeMemory.AlignedAlloc((nuint)sizeof(Amd64Context), Amd64Context.Alignment);
        *context = default;

        return context;
    }
}
