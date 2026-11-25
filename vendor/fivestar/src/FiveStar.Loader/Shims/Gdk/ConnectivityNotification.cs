// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>
/// One call of an XNetworkingConnectivityHintChangedCallback(void* context, const XNetworkingConnectivityHint*) on the
/// queue it was registered with; the hint lives in native memory for the duration of the call.
/// </summary>
internal sealed unsafe class ConnectivityNotification : AsyncStep
{
    private readonly ConnectivityService _service;
    private readonly nint _callback;
    private readonly nint _context;

    public ConnectivityNotification(ConnectivityService service, nint callback, nint context)
    {
        _service = service;
        _callback = callback;
        _context = context;
    }

    public override void Run(bool canceled)
    {
        if (canceled)
        {
            return;
        }

        byte* hint = (byte*)NativeMemory.AllocZeroed(NetworkingService.HintSize);

        try
        {
            _service.GetHint(hint);
            ((delegate* unmanaged<nint, byte*, void>)_callback)(_context, hint);
        }
        finally
        {
            NativeMemory.Free(hint);
        }
    }
}
