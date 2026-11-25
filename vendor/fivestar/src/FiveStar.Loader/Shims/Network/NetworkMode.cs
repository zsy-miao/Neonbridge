// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.Network;

/// <summary>What the game is told about the network (<c>--network</c>). Outbound traffic is sealed in every mode.</summary>
internal enum NetworkMode
{
    /// <summary>Connectivity level None: the game takes its offline paths.</summary>
    Offline,

    /// <summary>Internet access is reported (or the level of <c>--network-level</c>); every connection still fails.</summary>
    SealedOnline,
}
