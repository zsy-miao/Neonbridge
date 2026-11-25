// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X;

/// <summary>What a queue was asked to do, for <see cref="QueueProgress"/>.</summary>
internal enum QueueEventKind
{
    None,
    Execute,
    Signal,
    Wait,
}
