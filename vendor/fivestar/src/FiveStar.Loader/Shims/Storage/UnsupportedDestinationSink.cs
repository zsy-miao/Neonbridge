// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Storage;

/// <summary>The sink while no GPU destination is implemented: fails the request with E_NOTIMPL and logs the first one.</summary>
internal sealed class UnsupportedDestinationSink : IStorageDestinationSink
{
    private readonly LoaderLog _log;
    private int _logged;

    public UnsupportedDestinationSink(LoaderLog log)
    {
        _log = log;
    }

    public int Write(in StorageRequest request, ReadOnlySpan<byte> data)
    {
        if (Interlocked.Exchange(ref _logged, 1) == 0)
        {
            _log.Warn($"  [xcrdapi] DirectStorage request with options 0x{request.Options:x}: only system memory destinations are implemented");
        }

        return HResult.NotImplemented;
    }
}
