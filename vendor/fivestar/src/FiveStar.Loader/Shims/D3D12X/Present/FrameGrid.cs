// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X.Present;

/// <summary>
/// The frame origins: origin k (k from 1) falls at <see cref="Origin"/> + (k - 1) * <see cref="Interval"/>, in
/// Stopwatch timestamps. Replaced whole when the interval changes, so a waiting thread never sees half of it.
/// </summary>
internal sealed record FrameGrid(long Origin, long Interval, uint Microseconds, uint Period);
