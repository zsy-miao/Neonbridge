// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using Xunit;

namespace FiveStar.Shaders.Tests.Toolchain;

/// <summary>A test on real programs that compares with llvm-objdump: skipped as a real-program test, or without LLVM.</summary>
[AttributeUsage(AttributeTargets.Method)]
public sealed class LlvmProgramTheoryAttribute : TheoryAttribute
{
    public LlvmProgramTheoryAttribute()
    {
        Skip = new RealProgramTheoryAttribute().Skip ?? ShaderToolchain.MissingLlvmReason();
    }
}
