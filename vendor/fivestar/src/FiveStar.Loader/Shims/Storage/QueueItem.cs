// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.Storage;

/// <summary>
/// One queue entry. <see cref="Target"/> is the status array or the fence (a reference is held until the entry ran);
/// <see cref="Value"/> is the status index or the fence value.
/// </summary>
internal readonly record struct QueueItem(QueueItemKind Kind, long Sequence, StorageRequest Request, nint Target, ulong Value);
