// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>How a pipeline host's compile ended.</summary>
internal enum PipelineHostOutcome
{
    /// <summary>The host built the pipeline: the driver's cache holds it for the loader's own create.</summary>
    Built,

    /// <summary>The host passed its memory limit (or failed out of memory near it).</summary>
    KilledMemory,

    /// <summary>The host passed its time limit and was ended.</summary>
    KilledTime,

    /// <summary>The host did not start, or the driver refused the pipeline: the loader builds it itself.</summary>
    Failed,

    /// <summary>
    /// The host ended without an answer, short of its limits: the driver crashed in the compile (amdxc64 reads address 0
    /// when the commit charge is exhausted, issue #19), so the loader does not build it in its own process; it stays a
    /// stand-in for this boot, not recorded.
    /// </summary>
    Crashed,
}
