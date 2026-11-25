// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Calls;

/// <summary>Calls into libHttpClient.GDK.dll, each checked against its disassembly first.</summary>
internal static unsafe class HttpClientRecipes
{
    public const string Module = "libhttpclient.gdk.dll";

    /// <summary>
    /// What HCInitialize returns while the XGameRuntime ABI table is empty: QueryApiImpl answers REGDB_E_CLASSNOTREG,
    /// which libHttpClient reports through XErrorReport and maps to this value.
    /// </summary>
    private const int InitializeWithoutAbiTable = unchecked((int)0x89240107);

    /// <summary>HCGetLibVersion(const char** version): stores a pointer to a static string, returns S_OK.</summary>
    public static ExportCallResult LibVersion(ExportCallContext call)
    {
        nint version = 0;
        int hr = ((delegate* unmanaged<nint*, int>)call.Function)(&version);
        string? text = Marshal.PtrToStringAnsi(version);

        return new ExportCallResult($"hr=0x{hr:x8}, version=\"{text}\"", hr == HResult.Ok && !string.IsNullOrEmpty(text));
    }

    /// <summary>
    /// Not part of "all": it terminates the process by design. With an allocator that always fails, HCInitialize throws
    /// std::bad_alloc through _CxxThrowException in the mapped vcruntime140, the RtlPcToFileHeader shim, RaiseException
    /// and __CxxFrameHandler3; the exception reaches a noexcept function without a try block (libhttpclient+0x30d90),
    /// so std::terminate is the correct C++ outcome, and the terminate shim logs the native stack before fail-fast.
    /// </summary>
    public static ExportCallResult InitializeOutOfMemory(ExportCallContext call)
    {
        var setFunctions = call.Image.Exports.Find("HCMemSetFunctions", 0);
        var cleanup = call.Image.Exports.Find("HCCleanup", 0);

        if (setFunctions is null)
        {
            return new ExportCallResult("HCMemSetFunctions missing", false);
        }

        var set = (delegate* unmanaged<nint, nint, int>)setFunctions.Address;
        delegate* unmanaged<nuint, uint, nint> fail = &RecipeAllocatorCallbacks.AlwaysFail;
        delegate* unmanaged<nint, uint, void> nothing = &RecipeAllocatorCallbacks.FreeNothing;
        call.Runtime.RecipeAllocator.ResetFailures();
        int hits = call.Runtime.Counters.PcToFileHeaderHits;

        int hrSet = set((nint)fail, (nint)nothing);
        int hr = ((delegate* unmanaged<nint, int>)call.Function)(0);
        string extra = string.Empty;

        if (hr >= 0 && cleanup is not null)
        {
            ((delegate* unmanaged<void>)cleanup.Address)();
            extra = " (unexpected success; HCCleanup called)";
        }

        int hrRestore = set(0, 0);
        string summary = $"HCMemSetFunctions -> 0x{hrSet:x8}; HCInitialize -> 0x{hr:x8} (expect 0x8007000e E_OUTOFMEMORY){extra}; "
            + $"failing allocator called {call.Runtime.RecipeAllocator.FailedCalls}x; RtlPcToFileHeader shim hits +{call.Runtime.Counters.PcToFileHeaderHits - hits}; restore -> 0x{hrRestore:x8}";

        return new ExportCallResult(summary, hr == HResult.OutOfMemory);
    }

    /// <summary>
    /// Initializes with a switchable allocator, makes it fail for HCHttpCallCreate (whose try/catch is reachable
    /// without a noexcept frame between), expects E_OUTOFMEMORY, then cleans up. While the ABI table is empty,
    /// HCInitialize itself fails first.
    /// </summary>
    public static ExportCallResult CallCreateOutOfMemory(ExportCallContext call)
    {
        var exports = call.Image.Exports;
        var setFunctions = exports.Find("HCMemSetFunctions", 0);
        var initialize = exports.Find("HCInitialize", 0);
        var cleanup = exports.Find("HCCleanup", 0);
        var close = exports.Find("HCHttpCallCloseHandle", 0);

        if (setFunctions is null || initialize is null || cleanup is null || close is null)
        {
            return new ExportCallResult("required exports missing", false);
        }

        var set = (delegate* unmanaged<nint, nint, int>)setFunctions.Address;
        delegate* unmanaged<nuint, uint, nint> allocate = &RecipeAllocatorCallbacks.Allocate;
        delegate* unmanaged<nint, uint, void> free = &RecipeAllocatorCallbacks.Free;
        int hits = call.Runtime.Counters.PcToFileHeaderHits;
        int hrSet = set((nint)allocate, (nint)free);
        int hrInit = ((delegate* unmanaged<nint, int>)initialize.Address)(0);

        if (hrInit < 0)
        {
            set(0, 0);

            return new ExportCallResult($"HCInitialize failed 0x{hrInit:x8}", hrInit == InitializeWithoutAbiTable);
        }

        int hr = CreateWithFailingAllocator(call, close.Address);
        ((delegate* unmanaged<void>)cleanup.Address)();
        int hrRestore = set(0, 0);
        var allocator = call.Runtime.RecipeAllocator;
        string summary = $"set -> 0x{hrSet:x8}, HCInitialize -> 0x{hrInit:x8}; HCHttpCallCreate with failing allocator -> 0x{hr:x8} (expect 0x8007000e), "
            + $"failed allocs {allocator.FailedCalls}, RtlPcToFileHeader shim hits +{call.Runtime.Counters.PcToFileHeaderHits - hits}; HCCleanup done; restore -> 0x{hrRestore:x8}; live toggle-allocations {allocator.Live}";

        return new ExportCallResult(summary, hr == HResult.OutOfMemory);
    }

    private static int CreateWithFailingAllocator(ExportCallContext call, nint close)
    {
        var allocator = call.Runtime.RecipeAllocator;
        nint created = 0;
        allocator.SetFailing(true);
        allocator.ResetFailures();
        int hr = ((delegate* unmanaged<nint*, int>)call.Function)(&created);
        allocator.SetFailing(false);

        if (hr >= 0 && created != 0)
        {
            ((delegate* unmanaged<nint, int>)close)(created);
        }

        return hr;
    }
}
