// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.D3D12X.Present;

/// <summary>
/// The window the game made itself (RegisterClassExA, CreateWindowExA and ShowWindow, through the real user32): the
/// largest visible top-level window of this process that is not the console.
/// </summary>
internal static unsafe class GameWindowFinder
{
    public static nint Find()
    {
        var search = new WindowSearch
        {
            ProcessId = PresentNative.GetCurrentProcessId(),
            Console = PresentNative.GetConsoleWindow(),
        };

        _ = PresentNative.EnumWindows(&Visit, (nint)(&search));

        return search.Best;
    }

    [UnmanagedCallersOnly]
    private static int Visit(nint window, nint context)
    {
        try
        {
            var search = (WindowSearch*)context;
            uint process = 0;
            _ = PresentNative.GetWindowThreadProcessId(window, &process);

            if (process != search->ProcessId || window == search->Console || PresentNative.IsWindowVisible(window) == 0)
            {
                return 1;
            }

            PlaneRect client;

            if (PresentNative.GetClientRect(window, &client) == 0)
            {
                return 1;
            }

            long area = (long)(client.Right - client.Left) * (client.Bottom - client.Top);

            if (search->Best == 0 || area > search->BestArea)
            {
                search->Best = window;
                search->BestArea = area;
            }

            return 1;
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "GameWindowFinder.Visit");

            return HResult.Fail;
        }
    }
}
