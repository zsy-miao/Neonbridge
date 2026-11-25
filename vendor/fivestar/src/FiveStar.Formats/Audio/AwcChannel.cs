// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Audio;

/// <summary>One channel of a multichannel stream: the id of the chunkless stream that names it, and its format.</summary>
/// <param name="Id">The channel's stream id.</param>
/// <param name="Format">Samples, sample rate, headroom and codec.</param>
public sealed record AwcChannel(uint Id, AwcFormat Format);
