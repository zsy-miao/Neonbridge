// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Diagnostics;

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>Warns once per key, so a failure the game repeats for every pipeline fills one line.</summary>
internal sealed class OnceLog
{
    private readonly object _gate = new();
    private readonly HashSet<string> _seen = new(StringComparer.Ordinal);
    private readonly LoaderLog _log;

    public OnceLog(LoaderLog log)
    {
        _log = log;
    }

    public void Warn(string key, string message)
    {
        lock (_gate)
        {
            if (!_seen.Add(key))
            {
                return;
            }
        }

        _log.Warn(message);
    }
}
