// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Translation.Core;

/// <summary>
/// The wave operations translated code calls. They work in console lane numbers and are defined in rdna_prelude.hlsli
/// ("program waves"), natively or emulated for a hardware wave of another size.
/// </summary>
internal static class WaveOps
{
    /// <summary>This lane's program lane number.</summary>
    public const string Lane = "vw_lane()";

    /// <summary>The number of program lanes a lane loop visits.</summary>
    public const string Lanes = "vw_lanes()";

    /// <summary>vw_read(x, lane): x of a program lane; a lane missing from the hardware wave reads 0.</summary>
    public const string ReadLane = "vw_read";

    /// <summary>vw_ballot(bool): the lane mask as uint2 (x = lanes 0..31, y = lanes 32..63).</summary>
    public const string Ballot = "vw_ballot";

    /// <summary>vw_any(bool): true when the condition holds on any lane of the program wave.</summary>
    public const string AnyTrue = "vw_any";

    /// <summary>vw_count(bool): the number of program-wave lanes where the condition holds.</summary>
    public const string CountBits = "vw_count";

    /// <summary>The program lane number from SV_GroupIndex (mesh: a wave is 64 consecutive threads).</summary>
    public const string GroupLane = "g_vw_lane";

    /// <summary>
    /// The header line of an emulated program: its wave size and the narrowest hardware wave that gives console
    /// results (every wider one does too).
    /// </summary>
    public static string FloorLine(int wave, int floor)
    {
        return $"// program wave {wave}; console results on hardware waves >= {floor} lanes";
    }

    /// <summary>The second header line of a mesh program that needs its whole console wave: the first reason found.</summary>
    public static string NarrowerLine(string reason)
    {
        return $"// narrower hardware waves differ: {reason}";
    }

    /// <summary>The second header line of a fast-waves mesh program (TranslationOptions.FastWaves): approximate below its console wave, and why.</summary>
    public static string FastLine(string reason)
    {
        return $"{TranslationOptions.FastWavesComment}approximate on narrower hardware waves: {reason}";
    }

    /// <summary>D3D12's narrowest wave.</summary>
    public const int NarrowestHardwareWave = 4;
}
