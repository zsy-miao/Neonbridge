// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>
/// One call of <c>void XPackageInstallationProgressCallback(void* context, XPackageInstallationMonitorHandle monitor)</c>
/// (XPackage.h) from the monitor's task queue; not called when the queue was terminated first.
/// </summary>
internal sealed unsafe class ProgressChangedStep : AsyncStep
{
    private readonly nint _callback;
    private readonly nint _context;
    private readonly nint _monitor;

    public ProgressChangedStep(nint callback, nint context, nint monitor)
    {
        _callback = callback;
        _context = context;
        _monitor = monitor;
    }

    public override void Run(bool canceled)
    {
        if (canceled)
        {
            return;
        }

        ((delegate* unmanaged<nint, nint, void>)_callback)(_context, _monitor);
    }
}
