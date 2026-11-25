// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>XNetworkingQueryPreferredLocalUdpMultiplayerPortAsync: the result is one uint16_t.</summary>
internal sealed class PortQueryOperation : ManagedAsyncOperation
{
    public override AsyncWorkResult Work()
    {
        return new AsyncWorkResult(HResult.Ok, sizeof(ushort));
    }

    public override unsafe int Write(byte* buffer, nuint size)
    {
        return NetworkingService.QueryPreferredPort((ushort*)buffer);
    }
}
