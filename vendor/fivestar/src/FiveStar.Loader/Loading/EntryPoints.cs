// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Loading;

/// <summary>Calls an image's entry point: DllMain for DLLs, mainCRTStartup/WinMainCRTStartup for executables.</summary>
internal static unsafe class EntryPoints
{
    /// <summary>DllMain(PROCESS_ATTACH); a FALSE result fails the load.</summary>
    public static void Attach(LoadedImage image, LoaderLog log)
    {
        if (!image.Pe.IsDll || image.Pe.EntryPointRva == 0)
        {
            return;
        }

        var entry = (delegate* unmanaged<nint, uint, nint, int>)image.Va(image.Pe.EntryPointRva);
        log.Info($"  DllMain(0x{image.Base:x}, DLL_PROCESS_ATTACH) ...");
        int result = entry(image.Base, Win32Constants.DllProcessAttach, 0);
        image.EntryCalled = true;
        image.EntryResult = result;
        log.Info($"  DllMain -> {result}");

        if (result == 0)
        {
            throw new InvalidOperationException($"{image.Name}: DllMain(PROCESS_ATTACH) returned FALSE");
        }
    }

    /// <summary>DllMain(PROCESS_DETACH) for a DLL whose attach ran.</summary>
    public static void Detach(LoadedImage image)
    {
        if (!image.EntryCalled || !image.Pe.IsDll)
        {
            return;
        }

        var entry = (delegate* unmanaged<nint, uint, nint, int>)image.Va(image.Pe.EntryPointRva);
        entry(image.Base, Win32Constants.DllProcessDetach, 0);
    }

    /// <summary>Calls an executable image's entry point.</summary>
    public static int RunExecutable(LoadedImage image)
    {
        if (image.Pe.IsDll)
        {
            throw new InvalidOperationException("image is a DLL");
        }

        var entry = (delegate* unmanaged<int>)image.Va(image.Pe.EntryPointRva);

        return entry();
    }
}
