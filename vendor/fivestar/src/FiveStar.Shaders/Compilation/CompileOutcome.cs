// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Compilation;

/// <summary>
/// The outcome of one compile: whether the DXIL was written, whether dxc was stopped at its time limit, and dxc's
/// messages (empty when it printed none).
/// </summary>
public sealed record CompileOutcome(bool Succeeded, bool TimedOut, string Error)
{
    public static CompileOutcome Success { get; } = new(Succeeded: true, TimedOut: false, string.Empty);

    public static CompileOutcome Timeout { get; } = new(Succeeded: false, TimedOut: true, string.Empty);

    public static CompileOutcome Failure(string error)
    {
        return new CompileOutcome(Succeeded: false, TimedOut: false, error);
    }
}
