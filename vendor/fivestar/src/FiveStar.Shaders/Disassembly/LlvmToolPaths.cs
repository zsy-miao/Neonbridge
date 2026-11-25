// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Disassembly;

/// <summary>
/// Where llvm-mc and llvm-objdump live and which GPU they decode for. gfx1013 is the closest public LLVM target to the
/// Xbox Series GPU: RDNA with the ray-tracing instructions.
/// </summary>
public sealed record LlvmToolPaths(string Assembler, string Objdump, string Cpu)
{
    /// <summary>The default target CPU.</summary>
    public const string DefaultCpu = "gfx1013";

    /// <summary>Tool paths inside one LLVM bin folder.</summary>
    public static LlvmToolPaths InFolder(string binFolder)
    {
        var suffix = OperatingSystem.IsWindows() ? ".exe" : string.Empty;

        return new LlvmToolPaths(
            Path.Combine(binFolder, "llvm-mc" + suffix), Path.Combine(binFolder, "llvm-objdump" + suffix), DefaultCpu);
    }
}
