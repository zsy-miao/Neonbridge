// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Audio.Xma;

/// <summary>A decoded XMA2 sound.</summary>
/// <param name="Samples">Interleaved float PCM, full scale 1.0, trimmed (see <see cref="XmaTrackDecoder"/>).</param>
/// <param name="Channels">Channel count.</param>
/// <param name="SampleRate">Sample rate in Hz.</param>
/// <param name="Frames">XMA frames decoded, 512 samples per channel each.</param>
/// <param name="BadFrames">Frames that did not decode cleanly or were cut off; 0 for a sound that decoded cleanly.</param>
public sealed record XmaDecodedSound(float[] Samples, int Channels, int SampleRate, long Frames, long BadFrames);
