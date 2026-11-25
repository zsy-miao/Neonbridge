// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.Storage;

/// <summary>
/// Receives requests whose destination is not system memory (a GPU buffer, texture region or tiles). The queue worker
/// has already read and inflated the source: the data handed to <see cref="Write"/> holds the final bytes in staging
/// memory, valid only during the call. <see cref="StorageRequest.Raw"/> carries the destination fields for the implementer to decode.
/// </summary>
internal interface IStorageDestinationSink
{
    /// <summary>Writes <paramref name="data"/> to the request's destination; returns an HRESULT for the status array.</summary>
    int Write(in StorageRequest request, ReadOnlySpan<byte> data);
}
