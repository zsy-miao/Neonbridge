// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Tests.Cases;

/// <summary>
/// One thread of a compute case: flat thread id, index in its group, group, lane in its modelled wave, the modelled wave
/// size (the smaller of the PC and AMD wave sizes) and the flat ids of the threads in its wave (index = lane).
/// </summary>
public sealed record ComputeThread(int Id, int GroupIndex, int Group, int Lane, int WaveSize, int[] WaveThreads);
