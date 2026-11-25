// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Translation.Core;

/// <summary>
/// The bound on translated loops. A console program's loops end on the data the console gives them; on the PC a wrong
/// input (a NaN or huge float step count, a garbage loop bound) can keep one going until the driver removes the device
/// (docs/shader-translation.md#loop-guard). The cap counts the iterations of every structured loop of one invocation
/// together, so nested loops cannot multiply it; once spent, each loop leaves through its first exit. The block dispatch
/// stops after <see cref="DispatchSteps"/> blocks.
/// </summary>
public static class LoopGuard
{
    public const long DispatchSteps = 1 << 24;

    /// <summary>The dispatch form's block steps of <paramref name="stage"/> under <paramref name="caps"/>, as an HLSL literal.</summary>
    public static string DispatchHex(ShaderStage stage, LoopCaps caps)
    {
        return Hlsl.HexUint(caps.DispatchSteps(stage));
    }

    /// <summary>The blocks a thread of a superstep program of <paramref name="stage"/> may run under <paramref name="caps"/>, as an HLSL literal.</summary>
    public static string SuperstepHex(ShaderStage stage, LoopCaps caps)
    {
        return Hlsl.HexUint(caps.SuperstepSteps(stage));
    }

    /// <summary>The group rounds of a mesh program of <paramref name="stage"/> under <paramref name="caps"/>, as an HLSL literal.</summary>
    public static string RoundsHex(ShaderStage stage, LoopCaps caps)
    {
        return Hlsl.HexUint(caps.Rounds(stage));
    }

    /// <summary>Loop iterations one invocation of a <paramref name="stage"/> program may run under <paramref name="caps"/>, as an HLSL literal.</summary>
    public static string Hex(ShaderStage stage, LoopCaps caps)
    {
        return Hlsl.HexUint(caps.Iterations(stage));
    }
}
