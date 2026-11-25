// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims;

/// <summary>
/// System functions that must know the mapped images, which the OS loader does not. Bound for every importing module
/// ("*"), because they are imported from kernel32, ntdll or different api-ms-win sets.
/// </summary>
internal static unsafe partial class LoaderShims
{
    private const string UcrtLibrary = "ucrtbase.dll";
    private const string TerminateExport = "terminate";

    /// <summary>
    /// vcruntime's _CxxThrowException calls RtlPcToFileHeader(pThrowInfo) for the image base the x64 ThrowInfo RVAs are
    /// relative to. ntdll knows only OS-loaded modules, so for a mapped image it would return NULL and C++ throw/catch
    /// would crash. Effective for images whose imports the loader binds (a mapped vcruntime140.dll).
    /// </summary>
    [HotPath]
    [Shim("*", "RtlPcToFileHeader")]
    [UnmanagedCallersOnly]
    private static nint RtlPcToFileHeader(nint pc, nint* baseOfImage)
    {
        try
        {
            var runtime = ShimContext.Runtime;
            var image = runtime.Images.FindByAddress(pc);

            if (image is null)
            {
                return Ntdll.RtlPcToFileHeader(pc, baseOfImage);
            }

            runtime.Counters.CountPcToFileHeader();

            if (runtime.Log.DetailEnabled)
            {
                LogHit(runtime, pc, image);
            }

            *baseOfImage = image.Base;

            return image.Base;
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "LoaderShims.RtlPcToFileHeader");

            return default;
        }
    }

    private static void LogHit(LoaderRuntime runtime, nint pc, Loading.LoadedImage image)
    {
        runtime.Log.Detail($"  [loader] RtlPcToFileHeader(0x{pc:x}) -> {image.Name} @ 0x{image.Base:x}");
    }

    /// <summary>
    /// AddVectoredExceptionHandler from mapped code: registered as asked, then the loader's fault reporter (if
    /// installed) moves back in front, so it still sees each fault first. It only reports and continues the search.
    /// </summary>
    [Shim("*", "AddVectoredExceptionHandler")]
    [UnmanagedCallersOnly]
    private static nint AddVectoredExceptionHandler(uint first, nint handler)
    {
        try
        {
            var runtime = ShimContext.Runtime;
            nint handle = Kernel32.AddVectoredExceptionHandler(first, handler);
            runtime.Log.Detail($"  [loader] AddVectoredExceptionHandler({first}, {runtime.Images.Describe(handler)}) -> 0x{handle:x}");
            runtime.Faults.KeepFirst();

            return handle;
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "LoaderShims.AddVectoredExceptionHandler");

            return default;
        }
    }

    /// <summary>
    /// std::terminate from a mapped image (FH4 calls it for noexcept violations and unmatched throws) logs the native
    /// stack, then hands over to the real ucrtbase terminate, which fail-fasts.
    /// </summary>
    [Shim("*", "terminate")]
    [UnmanagedCallersOnly]
    private static void Terminate()
    {
        try
        {
            var runtime = ShimContext.Runtime;
            runtime.Log.Error("std::terminate() called from mapped code");
            NativeStack.Log(runtime.Images, runtime.Log, "terminate call stack");
            var real = (delegate* unmanaged<void>)NativeLibrary.GetExport(NativeLibrary.Load(UcrtLibrary), TerminateExport);
            real();
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "LoaderShims.Terminate");
        }
    }
}
