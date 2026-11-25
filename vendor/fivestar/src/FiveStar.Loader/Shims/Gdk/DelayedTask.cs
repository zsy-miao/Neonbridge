// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>A callback submitted with a delay: its timer enqueues it on the port when it fires.</summary>
internal sealed class DelayedTask
{
    private readonly TaskPort _port;
    private readonly TaskEntry _entry;
    private readonly TaskQueue _via;

    public DelayedTask(TaskPort port, TaskEntry entry, TaskQueue via)
    {
        _port = port;
        _entry = entry;
        _via = via;
        Timer = new Timer(Fire);
    }

    public Timer Timer { get; }

    private void Fire(object? state)
    {
        _port.FireDelayed(this, _entry, _via);
    }
}
