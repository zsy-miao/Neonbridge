// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.Storage;

/// <summary>What a queue entry does when the worker reaches it.</summary>
internal enum QueueItemKind
{
    /// <summary>Reads (and inflates) one request into its destination.</summary>
    Request,

    /// <summary>Completes a status array entry with the first failure since the previous status.</summary>
    Status,

    /// <summary>Signals a fence to a value.</summary>
    Signal,
}
