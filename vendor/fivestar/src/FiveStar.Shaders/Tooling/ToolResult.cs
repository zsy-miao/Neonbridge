// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Tooling;

/// <summary>The outcome of one external tool run. <see cref="TimedOut"/> runs were killed and have no exit code.</summary>
public sealed record ToolResult(int ExitCode, string Output, string Error, bool TimedOut);
