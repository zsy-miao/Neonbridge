// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.Intrinsics.X86;

namespace FiveStar.Loader.Cpu;

/// <summary>
/// Which of the AMD (Zen 2) instruction families the game uses the host CPU has. Bits from the AMD64 Architecture
/// Programmer's Manual, volume 3, appendix E (CPUID).
/// </summary>
internal readonly record struct CpuFeatures(bool Sse4a, bool Clzero, bool MonitorX, bool Clwb)
{
    /// <summary>CPUID Fn0000_0000 and Fn8000_0000: EAX is the highest standard and extended leaf.</summary>
    private const int StandardRange = 0;
    private const int ExtendedRange = unchecked((int)0x80000000);

    /// <summary>CPUID Fn8000_0001 ECX bit 6 SSE4A, bit 29 MONITORX.</summary>
    private const int ExtendedFeatures = unchecked((int)0x80000001);
    private const int Sse4aBit = 6;
    private const int MonitorXBit = 29;

    /// <summary>CPUID Fn8000_0008 EBX bit 0 CLZERO.</summary>
    private const int ExtendedIdentifiers = unchecked((int)0x80000008);
    private const int ClzeroBit = 0;

    /// <summary>CPUID Fn0000_0007 subleaf 0 EBX bit 24 CLWB (Intel SDM volume 2, CPUID).</summary>
    private const int StructuredFeatures = 7;
    private const int ClwbBit = 24;

    /// <summary>A host that has none of them, as an Intel CPU without CLWB; what <c>--cpu intel</c> reports.</summary>
    public static CpuFeatures None => default;

    /// <summary>Whether the host runs every one of them natively.</summary>
    public bool All => Sse4a && Clzero && MonitorX && Clwb;

    /// <summary>Reads the host CPU's CPUID leaves.</summary>
    public static CpuFeatures Detect()
    {
        if (!X86Base.IsSupported)
        {
            return None;
        }

        int maxStandard = X86Base.CpuId(StandardRange, 0).Eax;
        uint maxExtended = (uint)X86Base.CpuId(ExtendedRange, 0).Eax;
        int extendedEcx = HasLeaf(maxExtended, ExtendedFeatures) ? X86Base.CpuId(ExtendedFeatures, 0).Ecx : 0;
        int identifiersEbx = HasLeaf(maxExtended, ExtendedIdentifiers) ? X86Base.CpuId(ExtendedIdentifiers, 0).Ebx : 0;
        int structuredEbx = maxStandard >= StructuredFeatures ? X86Base.CpuId(StructuredFeatures, 0).Ebx : 0;

        return new CpuFeatures(
            Bit(extendedEcx, Sse4aBit),
            Bit(identifiersEbx, ClzeroBit),
            Bit(extendedEcx, MonitorXBit),
            Bit(structuredEbx, ClwbBit));
    }

    /// <summary>The families the host lacks, for the start-up line; empty when it has them all.</summary>
    public string Missing()
    {
        var missing = new List<string>();
        Add(missing, Sse4a, "SSE4a (INSERTQ, EXTRQ)");
        Add(missing, Clzero, "CLZERO");
        Add(missing, MonitorX, "MONITORX/MWAITX");
        Add(missing, Clwb, "CLWB");

        return string.Join(", ", missing);
    }

    private static void Add(List<string> missing, bool present, string name)
    {
        if (!present)
        {
            missing.Add(name);
        }
    }

    private static bool HasLeaf(uint maxExtended, int leaf)
    {
        return maxExtended >= (uint)leaf;
    }

    private static bool Bit(int register, int bit)
    {
        return ((register >> bit) & 1) != 0;
    }
}
