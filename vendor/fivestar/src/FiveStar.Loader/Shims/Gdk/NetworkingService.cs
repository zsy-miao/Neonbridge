// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>
/// XNetworking.h layouts and the preferred multiplayer port (the Xbox default). The connectivity hint itself comes from
/// <see cref="ConnectivityService"/>.
/// </summary>
internal static unsafe class NetworkingService
{
    public const string PortApi = "XNetworkingQueryPreferredLocalUdpMultiplayerPortAsync";

    public const string SecurityInformationApi = "XNetworkingQuerySecurityInformationForUrlAsync";

    /// <summary>UDP 3074, the port Xbox titles use for multiplayer traffic.</summary>
    public const ushort PreferredUdpPort = 3074;

    // struct XNetworkingConnectivityHint { level; cost; uint32_t ianaInterfaceType; bool networkInitialized,
    // approachingDataLimit, overDataLimit, roaming; }: 16 bytes.
    public const int HintSize = 16;
    public const int HintLevelOffset = 0;
    public const int HintCostOffset = 4;
    public const int HintInitializedOffset = 12;

    public static int QueryPreferredPort(ushort* port)
    {
        if (port == null)
        {
            return HResult.Pointer;
        }

        *port = PreferredUdpPort;

        return HResult.Ok;
    }
}
