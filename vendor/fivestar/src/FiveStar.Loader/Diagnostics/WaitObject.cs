// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using FiveStar.Loader.Loading;

namespace FiveStar.Loader.Diagnostics;

/// <summary>An event or semaphore of the game (<see cref="WaitObjects"/>): how it was made and when it was last signaled.</summary>
internal sealed class WaitObject
{
    /// <summary>Frames of the creating call shown: those in mapped images (the game and its manual modules).</summary>
    private const int ShownFrames = 6;

    private readonly object _gate = new();
    private readonly nint[] _frames;
    private uint _signaler;
    private double _signaledAt = -1;
    private long _signals;

    public WaitObject(string kind, uint creator, double createdAt, nint[] frames)
    {
        Kind = kind;
        Creator = creator;
        CreatedAt = createdAt;
        _frames = frames;
    }

    public string Kind { get; }

    public uint Creator { get; }

    public double CreatedAt { get; }

    public void Signal(uint thread, double at)
    {
        lock (_gate)
        {
            _signaler = thread;
            _signaledAt = at;
            _signals++;
        }
    }

    /// <summary>"auto-reset event made by thread T at S s from game+0x.. &lt;- ..; signaled N times, last ..".</summary>
    public string Describe(ImageTable images, double now)
    {
        var made = _frames.Where(frame => images.FindByAddress(frame) is not null).Take(ShownFrames).Select(images.Describe);
        string signal;

        lock (_gate)
        {
            signal = _signals == 0
                ? "never signaled"
                : string.Create(CultureInfo.InvariantCulture, $"signaled {_signals} times, last {now - _signaledAt:F1} s ago by thread {_signaler}");
        }

        return string.Create(CultureInfo.InvariantCulture, $"{Kind} made by thread {Creator} at {CreatedAt:F1} s from {string.Join(" <- ", made)}; {signal}");
    }
}
