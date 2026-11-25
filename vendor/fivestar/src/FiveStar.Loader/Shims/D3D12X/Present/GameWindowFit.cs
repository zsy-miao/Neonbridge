// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X.Present;

/// <summary>
/// The game sizes its window (WS_OVERLAPPEDWINDOW at CW_USEDEFAULT, 0x142478440) to the render size, frame included:
/// 3840x2160 on any monitor, so on a 4K screen the frame, caption and taskbar push part of it off the screen, and on a
/// smaller one most of it. At the first present a window larger than its monitor's work area is made the largest one
/// whose client area has the plane's aspect ratio and fits there, centered. The swap chain then scales into whatever
/// client area the window has (<see cref="PresentScaler"/>).
/// </summary>
internal static unsafe class GameWindowFit
{
    /// <summary>MONITOR_DEFAULTTONEAREST.</summary>
    private const uint NearestMonitor = 2;

    /// <summary>SWP_NOZORDER | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS: the window's thread applies it, this one goes on.</summary>
    private const uint MoveFlags = 0x0004 | 0x0010 | 0x4000;

    /// <summary>The new window rectangle (left, top, width, height), or null when the window already fits.</summary>
    public static (int X, int Y, int Width, int Height)? Fit(nint window, uint planeWidth, uint planeHeight)
    {
        PlaneRect outer;
        PlaneRect client;
        var info = new MonitorInfo { Size = (uint)sizeof(MonitorInfo) };
        nint monitor = PresentNative.MonitorFromWindow(window, NearestMonitor);

        if (PresentNative.GetWindowRect(window, &outer) == 0 || PresentNative.GetClientRect(window, &client) == 0
            || monitor == 0 || PresentNative.GetMonitorInfoW(monitor, &info) == 0)
        {
            return null;
        }

        var rect = Place(outer, client, info.Work, planeWidth, planeHeight);

        if (rect.HasValue)
        {
            var (x, y, width, height) = rect.Value;
            _ = PresentNative.SetWindowPos(window, 0, x, y, width, height, MoveFlags);
        }

        return rect;
    }

    /// <summary>Where a window of rectangle <paramref name="outer"/> and client size <paramref name="client"/> goes; null when it fits <paramref name="work"/>.</summary>
    public static (int X, int Y, int Width, int Height)? Place(PlaneRect outer, PlaneRect client, PlaneRect work, uint planeWidth, uint planeHeight)
    {
        int workWidth = work.Right - work.Left;
        int workHeight = work.Bottom - work.Top;
        int outerWidth = outer.Right - outer.Left;
        int outerHeight = outer.Bottom - outer.Top;

        if (outerWidth <= workWidth && outerHeight <= workHeight)
        {
            return null;
        }

        int frameWidth = outerWidth - (client.Right - client.Left);
        int frameHeight = outerHeight - (client.Bottom - client.Top);
        var inside = Letterbox.Fit(planeWidth, planeHeight, (uint)Math.Max(1, workWidth - frameWidth), (uint)Math.Max(1, workHeight - frameHeight));
        int width = inside.Width + frameWidth;
        int height = inside.Height + frameHeight;

        return (work.Left + ((workWidth - width) / Letterbox.Halves), work.Top + ((workHeight - height) / Letterbox.Halves), width, height);
    }
}
