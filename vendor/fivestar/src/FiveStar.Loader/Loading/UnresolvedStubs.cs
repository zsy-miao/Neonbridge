// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Codegen;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Shims;

namespace FiveStar.Loader.Loading;

/// <summary>
/// Generates a native stub per unresolved or not-implemented import: <c>mov rdx,[rsp]; mov rcx,id; mov rax,handler; jmp rax</c>.
/// The handler logs the first call and returns the stub's value (or terminates in trap mode). The caller's arguments
/// are discarded, which is all a "not implemented" stub needs.
/// </summary>
internal sealed unsafe class UnresolvedStubs
{
    private readonly object _gate = new();
    private readonly List<UnresolvedStub> _stubs = [];
    private readonly Dictionary<(string Name, long ReturnValue, bool NotImplemented), nint> _cache = [];
    private readonly CodeHeap _heap;
    private readonly ImageTable _images;
    private readonly UnresolvedMode _mode;
    private readonly LoaderLog _log;
    private readonly Action<string> _failFast;
    private int _callCount;
    private int _notImplementedCallCount;

    public UnresolvedStubs(CodeHeap heap, ImageTable images, UnresolvedMode mode, LoaderLog log, Action<string> failFast)
    {
        _heap = heap;
        _images = images;
        _mode = mode;
        _log = log;
        _failFast = failFast;
    }

    /// <summary>Calls that reached a stub for an import nothing resolved.</summary>
    public int CallCount => Volatile.Read(ref _callCount);

    /// <summary>Calls that reached a stub for a known but deliberately unimplemented function.</summary>
    public int NotImplementedCallCount => Volatile.Read(ref _notImplementedCallCount);

    /// <summary>Calls to the stubs named "module!symbol".</summary>
    public int CallsTo(string moduleBangSymbol)
    {
        lock (_gate)
        {
            return _stubs.Where(s => s.Name.Equals(moduleBangSymbol, StringComparison.OrdinalIgnoreCase)).Sum(s => s.Calls);
        }
    }

    /// <summary>
    /// Returns the stub for an import. Stubs are cached per (name, value, kind), so an IAT slot and a later
    /// GetProcAddress for the same function share one stub and one first-call log line. <paramref name="sealNow"/>
    /// makes a stub handed out at run time executable at once.
    /// </summary>
    public nint Create(string module, string symbol, long returnValue, bool notImplemented, bool sealNow)
    {
        string name = $"{module}!{symbol}";
        var key = (name.ToLowerInvariant(), returnValue, notImplemented);
        int id;

        lock (_gate)
        {
            if (_cache.TryGetValue(key, out var existing))
            {
                return existing;
            }

            id = _stubs.Count;
            _stubs.Add(new UnresolvedStub(name, returnValue, notImplemented));
        }

        nint address = WriteStub(id);

        if (sealNow)
        {
            _heap.Seal();
        }

        lock (_gate)
        {
            _cache[key] = address;
        }

        return address;
    }

    /// <summary>Runs when a stub is called: counts it, logs the first call (with its caller), and returns its value or traps.</summary>
    public nint OnCalled(int id, nint returnAddress)
    {
        UnresolvedStub stub;

        lock (_gate)
        {
            stub = _stubs[id];
        }

        int calls = stub.CountCall();

        if (stub.NotImplemented)
        {
            return OnNotImplemented(stub, calls, returnAddress);
        }

        Interlocked.Increment(ref _callCount);

        if (calls == 1)
        {
            _log.Warn($"UNRESOLVED IMPORT CALLED: {stub.Name} from {_images.Describe(returnAddress)} (returning 0x{stub.ReturnValue:x}; further calls not logged)");
        }

        if (_mode == UnresolvedMode.Trap)
        {
            _log.Error($"trap mode: terminating on call to unresolved {stub.Name}");
            _failFast($"fivestar-loader: call to unresolved import {stub.Name}");
        }

        return (nint)stub.ReturnValue;
    }

    [HotPath]
    [UnmanagedCallersOnly]
    private static nint StubCalled(nint id, nint returnAddress)
    {
        try
        {
            return ShimContext.Runtime.Stubs.OnCalled((int)id, returnAddress);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "UnresolvedStubs.StubCalled");

            return default;
        }
    }

    private nint OnNotImplemented(UnresolvedStub stub, int calls, nint returnAddress)
    {
        Interlocked.Increment(ref _notImplementedCallCount);

        if (calls == 1)
        {
            _log.Warn($"NOT IMPLEMENTED: {stub.Name} -> 0x{stub.ReturnValue:x} from {_images.Describe(returnAddress)} (logged once)");
        }

        return (nint)stub.ReturnValue;
    }

    private nint WriteStub(int id)
    {
        delegate* unmanaged<nint, nint, nint> handler = &StubCalled;
        var code = new X64Emitter();
        code.LoadRdxFromStackTop();
        code.MovRcx((ulong)id);
        code.MovRax((ulong)handler);
        code.JmpRax();

        return _heap.Write(code.ToArray());
    }
}
