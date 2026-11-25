// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>
/// Turns a managed exception caught at the native boundary into a logged error and an HRESULT: an
/// [UnmanagedCallersOnly] method must not let one escape.
/// </summary>
internal sealed class NativeBoundary
{
    private readonly LoaderLog _log;

    public NativeBoundary(LoaderLog log)
    {
        _log = log;
    }

    public bool TraceEnabled => _log.DetailEnabled;

    /// <summary>Logs the exception of <paramref name="api"/> and returns E_FAIL.</summary>
    public int Failed(string api, Exception exception)
    {
        Report(api, exception);

        return HResult.Fail;
    }

    /// <summary>Logs the exception of a function without a result.</summary>
    public void Report(string api, Exception exception)
    {
        _log.Error($"[gdk] {api} threw {exception.GetType().Name}: {exception.Message}");
    }

    public void Warn(string message)
    {
        _log.Warn($"[gdk] {message}");
    }

    /// <summary>A line of the log, for notable events such as initialization.</summary>
    public void Info(string message)
    {
        _log.Info($"  [gdk] {message}");
    }

    public void Error(string message)
    {
        _log.Error($"[gdk] {message}");
    }

    public void Trace(string message)
    {
        _log.Detail($"  [gdk] {message}");
    }
}
