// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Cli.Run;

/// <summary>The outcome of one prerequisite check; <see cref="Failed"/> stops <c>fivestar run</c>.</summary>
internal enum CheckStatus
{
    Ok,
    Warning,
    Failed,
}
