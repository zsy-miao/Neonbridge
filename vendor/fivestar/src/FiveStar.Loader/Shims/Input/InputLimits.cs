// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.Input;

/// <summary>Sizes and intervals of the shim's own GameInput implementation (not from the header).</summary>
internal static class InputLimits
{
    /// <summary>Keys one keyboard reading holds; reported as maxSimultaneousKeys.</summary>
    public const int MaxKeys = 16;

    /// <summary>Readings kept for GetNextReading and GetPreviousReading, across all devices.</summary>
    public const int HistoryDepth = 256;

    /// <summary>Sources are polled at most this often; calls in between return the readings already taken.</summary>
    public const long PollIntervalMicroseconds = 1000;

    /// <summary>An empty XInput slot is probed again after this long (XInputGetState on an empty slot is slow).</summary>
    public const long ProbeIntervalMicroseconds = 1_000_000;

    public const long MicrosecondsPerSecond = 1_000_000;

    /// <summary>The virtual-key code space of GetAsyncKeyState.</summary>
    public const int VirtualKeyCount = 256;

    /// <summary>XUSER_MAX_COUNT (XInput.h).</summary>
    public const int XInputSlots = 4;
}
