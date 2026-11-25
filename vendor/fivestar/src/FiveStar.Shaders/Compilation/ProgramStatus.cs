// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Compilation;

/// <summary>The status of one program in a corpus run (the values written to results.json).</summary>
public static class ProgramStatus
{
    /// <summary>Translated and compiled.</summary>
    public const string Ok = "ok";

    /// <summary>A mesh shader over the PC output limits, compiled through the compute + vertex-pulling fallback.</summary>
    public const string OkMeshFallback = "ok-msfb";

    /// <summary>Not selected (unknown hardware stage or filtered out by --stages).</summary>
    public const string SkipStage = "skip-stage";

    /// <summary>The FSSM header could not be decoded.</summary>
    public const string HeaderFailure = "regs-fail";

    public const string DisassemblyFailure = "disasm-fail";

    /// <summary>The translator does not handle an instruction or shape of the program.</summary>
    public const string Unsupported = "unsupported";

    /// <summary>The translator failed unexpectedly.</summary>
    public const string Crash = "crash";

    public const string CompileFailure = "dxc-fail";

    public const string CompileTimeout = "dxc-timeout";

    /// <summary>True for the statuses a retry run keeps.</summary>
    public static bool IsFinal(string? status)
    {
        return status is Ok or OkMeshFallback or SkipStage;
    }
}
