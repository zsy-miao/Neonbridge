// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.Network;

/// <summary>The network the game is shown: the mode and the connectivity level the hint reports.</summary>
/// <param name="Mode">Offline, or sealed online.</param>
/// <param name="Level">The reported level; null means the mode's own (None offline, InternetAccess sealed online).</param>
internal sealed record NetworkOptions(NetworkMode Mode, ConnectivityLevel? Level)
{
    public static NetworkOptions Offline { get; } = new(NetworkMode.Offline, null);

    /// <summary>The level XNetworkingGetConnectivityHint reports.</summary>
    public ConnectivityLevel ReportedLevel
    {
        get
        {
            if (Level is not null)
            {
                return Level.Value;
            }

            if (Mode == NetworkMode.SealedOnline)
            {
                return ConnectivityLevel.InternetAccess;
            }

            return ConnectivityLevel.None;
        }
    }
}
