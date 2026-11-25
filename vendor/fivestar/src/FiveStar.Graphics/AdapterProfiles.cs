// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Graphics;

/// <summary>
/// Synthetic adapters for <c>fivestar shader plan --profile</c>: GPUs not in this machine, described by what their
/// drivers report for D3D12_OPTIONS1.WaveLaneCountMin/Max and the mesh-shader tier. Each value names its source or says
/// it is an assumption. A synthetic adapter has index -1.
/// </summary>
public static class AdapterProfiles
{
    private const int Synthetic = -1;
    private const string ShaderModel = "6.8";
    private const string MeshTier1 = "Tier1";

    /// <summary>
    /// NVIDIA Ada Lovelace (RTX 40). Warps are 32 threads (CUDA C++ Programming Guide, "SIMT Architecture"), so 32-32;
    /// mesh shaders tier 1 since Turing (DirectX 12 Ultimate). Measured on an RTX 4090: SM 6.8, 32-32, Tier1.
    /// </summary>
    public static readonly AdapterInfo Ada = new(Synthetic, "ada (NVIDIA RTX 40)", false, ShaderModel, 32, 32, MeshTier1);

    /// <summary>
    /// AMD RDNA 2 (RX 6000, Steam Deck, Ryzen 6000 iGPUs). The compute units run wave32 and wave64 ("AMD RDNA 2
    /// Instruction Set Architecture" reference guide), so 32-64; mesh shaders tier 1 on the discrete RX 6000 parts
    /// (DirectX 12 Ultimate). Measured on a Ryzen iGPU: SM 6.8, 32-64, but no mesh shaders, so iGPUs may differ.
    /// </summary>
    public static readonly AdapterInfo Rdna2 = new(Synthetic, "rdna2 (AMD RX 6000)", false, ShaderModel, 32, 64, MeshTier1);

    /// <summary>
    /// Intel Xe2 (Lunar Lake Arc 140V/130V, Battlemage). Xe2's vector engines are natively SIMD16 and also run SIMD32
    /// (Intel's Xe2 architecture disclosures), so 16-32; Intel reports Max 32 (Dawn uses it as the subgroup maximum on
    /// Intel because it is reliable there). Min 16 rather than 8 is an assumption from the SIMD16 width. Mesh shaders
    /// tier 1: Arc is DirectX 12 Ultimate. SM 6.8 is an assumption (current drivers).
    /// </summary>
    public static readonly AdapterInfo IntelXe2 = new(Synthetic, "intel-xe2 (Lunar Lake Arc 140V)", false, ShaderModel, 16, 32, MeshTier1);

    /// <summary>
    /// Intel Xe-LPG / Xe-HPG (Meteor Lake Arc, Arc A-series). Their EUs run SIMD8, SIMD16 and SIMD32 shaders, so 8-32
    /// (an assumption from the compiler's SIMD widths); mesh shaders tier 1 (DirectX 12 Ultimate); SM 6.8 assumed.
    /// </summary>
    public static readonly AdapterInfo IntelXeLpg = new(Synthetic, "intel-xe-lpg (Meteor Lake Arc)", false, ShaderModel, 8, 32, MeshTier1);

    /// <summary>Every profile by its command-line name.</summary>
    public static readonly IReadOnlyDictionary<string, AdapterInfo> All = new Dictionary<string, AdapterInfo>(StringComparer.OrdinalIgnoreCase)
    {
        ["ada"] = Ada,
        ["rdna2"] = Rdna2,
        ["intel-xe2"] = IntelXe2,
        ["intel-xe-lpg"] = IntelXeLpg,
    };

    /// <summary>The profile of a name, or null.</summary>
    public static AdapterInfo? Find(string name)
    {
        return All.GetValueOrDefault(name);
    }
}
