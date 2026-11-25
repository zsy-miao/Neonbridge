// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Cli.Shader;

/// <summary>
/// How a sweep runs: pipelines created at once, the time one creation may take and the private memory the process may
/// use before it stops starting pipelines (and ends one that alone exceeds it).
/// </summary>
internal sealed record PsoLimits(int Jobs, TimeSpan Timeout, long MemoryBytes);
