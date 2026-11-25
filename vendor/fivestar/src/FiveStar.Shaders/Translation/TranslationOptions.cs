// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Translation;

/// <summary>
/// Translator choices. <paramref name="StructuredControlFlow"/> emits branches as if/else and loops instead of the
/// block dispatch loop, where the program's control-flow graph allows it. <paramref name="PinWaveSize"/> gives a compute
/// program that moves data between lanes by lane number <c>[WaveSize(n)]</c> with its native wave size, so it runs only
/// where the GPU offers that size instead of computing wrong results. <paramref name="EmulateWaveSize"/> makes the wave
/// operations of pixel, vertex and mesh programs keep console lane numbers on any hardware wave size (rdna_prelude.hlsli,
/// "program waves"); without it they use the hardware wave as it is. <see cref="Loops"/> bounds translated loops
/// per stage (<see cref="Core.LoopCaps"/>); null keeps the translator's own caps. <see cref="RoundsPerStop"/>, when set,
/// bounds a superstep program's group rounds at that many per wave-operation stop of its code (at least
/// <see cref="MinimumRounds"/>), below the caps' rounds: a large program's rounds cost so much that
/// the caps' bound comes after the driver's timeout.
/// </summary>
public sealed record TranslationOptions(bool StructuredControlFlow, bool PinWaveSize, bool EmulateWaveSize)
{
    /// <summary>The fewest rounds a per-stop budget gives (<see cref="RoundsPerStop"/>).</summary>
    public const long MinimumRounds = 256;

    /// <summary>The comment a superstep program starts its body with: its stops and its round bound (the loader logs it).</summary>
    public const string SuperstepCostComment = "  // superstep: ";

    /// <summary>The header line of a program that runs approximately under <see cref="FastWaves"/> (the loader logs it).</summary>
    public const string FastWavesComment = "// fast waves: ";

    public Core.LoopCaps? Loops { get; init; }

    public int? RoundsPerStop { get; init; }

    /// <summary>
    /// The superstep form passes registers between its dispatch cases through carry slots, by liveness
    /// (<see cref="Waves.SuperstepCarry"/>), in a tree of small switches, instead of keeping every register live around
    /// one switch. On by default; false gives the plain form.
    /// </summary>
    public bool SuperstepCarry { get; init; } = true;

    /// <summary>
    /// Fast waves, approximate (off by default): a mesh program the window analysis rejects keeps the native form on
    /// narrower hardware waves when it would pass with each hardware wave's <c>readfirstlane</c> taken for its console
    /// wave's and its branches on "any lane" decided per hardware wave (a skipped path still may change nothing read
    /// after it, or every hardware wave takes it), and it reads no other lane by number (<c>vw_read</c>, <c>vw_count</c>,
    /// native wave operations, shared LDS). It then needs no superstep form; its header says so (<see cref="FastWavesComment"/>).
    /// </summary>
    public bool FastWaves { get; init; }

    /// <summary>
    /// Typed image views (off by default): an image load, gather or store reads the view's component class from its
    /// T# record's dword 3 (bits 0-1: 0 float, 1 uint, 2 sint, which the loader writes with <c>--typed-image-views</c>) and
    /// declares the texture as <c>uint4</c> or <c>int4</c> for an integer view, whose bits a <c>float4</c> declaration
    /// leaves undefined (the main depth's stencil plane, X32_TYPELESS_G8X24_UINT, which the lighting classifies pixels by).
    /// A d16 load of an integer view keeps the low 16 bits of each value instead of converting a float.
    /// </summary>
    public bool TypedImageViews { get; init; }
}
