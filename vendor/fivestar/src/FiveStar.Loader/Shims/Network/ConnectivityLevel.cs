// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.Network;

/// <summary>XNetworkingConnectivityLevelHint (XNetworking.h, PC GDK 261000).</summary>
internal enum ConnectivityLevel : uint
{
    Unknown = 0,
    None = 1,
    LocalAccess = 2,
    InternetAccess = 3,
    ConstrainedInternetAccess = 4,
}
