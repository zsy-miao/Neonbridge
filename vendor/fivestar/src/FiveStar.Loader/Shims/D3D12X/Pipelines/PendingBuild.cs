// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>
/// The build of a pipeline whose programs became ready (<see cref="StandInPipelines.Waiting"/>): the creation, whether it
/// goes to the heavy thread, its name and what is told the built pipeline.
/// </summary>
internal sealed record PendingBuild(Func<PipelineResult> Create, bool Heavy, string What, Action<nint> Built);
