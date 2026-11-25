// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>
/// XLauncher.h and XLaunchRestartOnCrash: a web address opens in the PC's browser; any other URI (Xbox app and store
/// protocols) is logged and dropped. Crash restarts are recorded, not acted on; launching another game is logged only.
/// </summary>
internal sealed class LauncherService
{
    private readonly NativeBoundary _boundary;
    private string? _restartArguments;

    public LauncherService(NativeBoundary boundary)
    {
        _boundary = boundary;
    }

    /// <summary>The arguments of the last XLaunchRestartOnCrash, or null.</summary>
    public string? RestartArguments => Volatile.Read(ref _restartArguments);

    public int LaunchUri(string? text)
    {
        if (string.IsNullOrEmpty(text) || !Uri.TryCreate(text, UriKind.Absolute, out var uri))
        {
            return HResult.InvalidArgument;
        }

        if (uri.Scheme != Uri.UriSchemeHttps && uri.Scheme != Uri.UriSchemeHttp)
        {
            _boundary.Info($"XLaunchUri({text}) dropped (no handler offline)");

            return HResult.Ok;
        }

        _boundary.Info($"XLaunchUri({text}) -> browser");
        using var process = Process.Start(new ProcessStartInfo(uri.AbsoluteUri) { UseShellExecute = true });

        return HResult.Ok;
    }

    public int RestartOnCrash(string? arguments)
    {
        Volatile.Write(ref _restartArguments, arguments ?? string.Empty);
        _boundary.Info($"XLaunchRestartOnCrash(\"{arguments}\") recorded; the loader does not restart the game");

        return HResult.Ok;
    }

    /// <summary>XLaunchNewGame has no result; the call is logged and the current game keeps running.</summary>
    public void LaunchNewGame(string? exePath, string? arguments)
    {
        _boundary.Warn($"XLaunchNewGame(\"{exePath}\", \"{arguments}\") not supported; the game keeps running");
    }
}
