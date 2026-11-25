// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using FiveStar.Shaders.Hardware;

namespace FiveStar.Shaders.Translation.Core;

/// <summary>
/// Registers read before any write are hardware-preloaded inputs. Their meaning comes from the FSSM header and the stage
/// (docs/fssm.md): user data, system SGPRs, PS interpolation/position VGPRs, vertex and thread ids. Each one becomes an
/// assignment in the prologue.
/// </summary>
internal sealed class ImplicitInputs
{
    private const string NotPreloaded = "0u /* not preloaded */";
    private const string Components = "xyzw";

    // Merged ES/GS and NGG layouts: s0..s7 are system SGPRs, user data starts at s8; s3 holds the wave info;
    // the GS ring table pointer is s[8:9].
    private const int MergedUserBase = 8;
    private const int WaveInfoSgpr = 3;
    private const int RingTableLo = 8;
    private const int RingTableHi = 9;
    private const int CopyShaderRingTableHi = 1;

    // gfx10 hardware VS VGPRs: v0 VertexID, v3 InstanceID. NGG: v5 is the ES vertex id. GS: v0..v4 (see Geometry).
    private const int VertexIdVgpr = 0;
    private const int InstanceIdVgpr = 3;
    private const int NggVertexIdVgpr = 5;
    private const int GsVertexPair01 = 0;
    private const int GsVertexPair23 = 1;
    private const int GsPrimitiveId = 2;
    private const int GsVertexPair45 = 3;
    private const int GsInstanceId = 4;
    private const int GsVertex2 = 2;
    private const int GsVertex3 = 3;
    private const int GsVertex4 = 4;
    private const int GsVertex5 = 5;

    // Pixel inputs the shader reads as system values; the persp_center pair (I, J) are the weights of vertices 1 and 2.
    private static readonly IReadOnlyDictionary<string, string> PixelSystemInputs = new Dictionary<string, string>
    {
        ["pos_x"] = "asuint(in_pos.x)",
        ["pos_y"] = "asuint(in_pos.y)",
        ["pos_z"] = "asuint(in_pos.z)",
        ["pos_w"] = "asuint(in_pos.w)",
        ["front_face"] = "(in_front ? 1u : 0u)",
        ["ancillary"] = "(in_sample << 8)",
        ["sample_coverage"] = "in_coverage",
    };

    // Layout assumed when the header has no pixel block: barycentrics in v0/v1, position in v2..v5.
    private static readonly IReadOnlyList<PixelInput> DefaultPixelLayout =
    [
        new("persp_center", [0, 1]), new("pos_x", [2]), new("pos_y", [3]), new("pos_z", [4]), new("pos_w", [5]),
    ];

    private readonly TranslationState _state;

    public ImplicitInputs(TranslationState state)
    {
        _state = state;
    }

    /// <summary>Adds the prologue assignment of a register read before it was written.</summary>
    public void Load(RegisterKey key)
    {
        string value;

        if (key.Kind == RegisterKey.Scalar)
        {
            value = ScalarInput(key.Index);
        }
        else
        {
            value = VectorInput(key.Index);
        }

        _state.Touch(key);
        _state.Prologue.Add($"  {key.Name} = {value};");
    }

    private string ScalarInput(int sgpr)
    {
        var user = _state.Config.UserSgprs;

        if (sgpr >= SpecialRegisters.FirstSpecial)
        {
            // VCC, M0, ... read before any write on this path: loop-carried or undefined.
            return "0u /* special SGPR not preloaded */";
        }

        if (_state.IsCopyShader)
        {
            // Copy shader: s[0:1] is the driver ring table (the GSVS base reads as 0 in the private-LDS model).
            var copyValue = sgpr == CopyShaderRingTableHi ? "RING_TAG" : "0u";

            return copyValue;
        }

        switch (_state.Stage)
        {
            case ShaderStage.Geometry:
                return GeometryScalar(sgpr, user);
            case ShaderStage.Compute:
                return ComputeScalar(sgpr, user);
            case ShaderStage.Pixel:
                return PixelScalar(sgpr, user);
            case ShaderStage.Mesh:
                return MeshScalar(sgpr, user);
            default:
                return UserData(sgpr, user);
        }
    }

    private static string GeometryScalar(int sgpr, int? user)
    {
        // Merged ES/GS system SGPRs: s3 bits 8..15 = GS thread count (a full wave), s[8:9] = ring table.
        if (sgpr == WaveInfoSgpr)
        {
            return "(64u << 8)";
        }

        if (sgpr == RingTableLo)
        {
            return "0u /* ring table lo */";
        }

        if (sgpr == RingTableHi)
        {
            return "RING_TAG";
        }

        if (sgpr < MergedUserBase || (user is not null && sgpr >= MergedUserBase + user.Value))
        {
            return "0u";
        }

        return UserSgprRead(sgpr - MergedUserBase);
    }

    private string ComputeScalar(int sgpr, int? user)
    {
        var compute = _state.Config.Compute;

        if (compute is not null)
        {
            foreach (var groupId in compute.GroupIdSgprs)
            {
                if (groupId.Sgpr == sgpr)
                {
                    return $"in_gid.{groupId.Axis}";
                }
            }
        }

        return UserData(sgpr, user);
    }

    private string PixelScalar(int sgpr, int? user)
    {
        var pixel = _state.Config.Pixel;

        if (pixel is not null && sgpr == pixel.PrimMaskSgpr)
        {
            // PRIM_MASK only feeds M0 for v_interp, which this model does not need.
            return "0u";
        }

        return UserData(sgpr, user);
    }

    private static string MeshScalar(int sgpr, int? user)
    {
        // NGG system SGPR s3: bits 24..27 = wave index in the group (wave64 virtual layout).
        if (sgpr == WaveInfoSgpr)
        {
            return "((in_gi >> 6) << 24)";
        }

        if (sgpr < MergedUserBase)
        {
            return "0u";
        }

        if (user is not null && sgpr >= MergedUserBase + user.Value)
        {
            return "0u /* not preloaded (path-dependent read of an unset register) */";
        }

        return UserSgprRead(sgpr - MergedUserBase);
    }

    private string UserData(int sgpr, int? user)
    {
        var popsPixel = _state.Stage == ShaderStage.Pixel && (_state.Config.Pixel?.Pops ?? false);

        if (user is not null && sgpr >= user.Value && !popsPixel)
        {
            // Not preloaded by hardware: a path-dependent read of an unset register (e.g. the high half of a pair).
            return NotPreloaded;
        }

        return UserSgprRead(sgpr);
    }

    private static string UserSgprRead(int index)
    {
        return $"user_sgpr({index.ToString(CultureInfo.InvariantCulture)})";
    }

    private string VectorInput(int vgpr)
    {
        switch (_state.Stage)
        {
            case ShaderStage.Vertex:
                return VertexVector(vgpr);
            case ShaderStage.Pixel:
                return PixelVector(vgpr);
            case ShaderStage.Compute:
                return ComputeVector(vgpr);
            case ShaderStage.Mesh:
                return MeshVector(vgpr);
            case ShaderStage.Geometry:
                return GeometryVector(vgpr);
            default:
                throw new UnsupportedProgramException($"implicit input v{vgpr} in {ShaderStages.ProfileName(_state.Stage)}");
        }
    }

    private string VertexVector(int vgpr)
    {
        if (vgpr == VertexIdVgpr)
        {
            _state.Inputs[vgpr] = "vid";

            return "in_vid";
        }

        if (vgpr == InstanceIdVgpr)
        {
            _state.Inputs[vgpr] = "iid";

            return "in_iid";
        }

        if (_state.UsesFetchShader)
        {
            foreach (var entry in _state.Config.Vertex?.FetchInputs ?? [])
            {
                if (entry.Vgpr <= vgpr && vgpr < entry.Vgpr + entry.ComponentCount)
                {
                    // Fetch-shader output: one component of vertex attribute entry.Attribute (bound as ATTR<n>).
                    _state.FetchInputs.Add((entry.Attribute, entry.ComponentCount));

                    return $"asuint(fin.a{entry.Attribute}.{Components[vgpr - entry.Vgpr]})";
                }
            }
        }

        throw new UnsupportedProgramException($"implicit input v{vgpr} in vs");
    }

    private string PixelVector(int vgpr)
    {
        var layout = _state.Config.Pixel?.Inputs ?? DefaultPixelLayout;

        foreach (var input in layout)
        {
            if (input.Vgprs.Contains(vgpr) && PixelSystemInputs.TryGetValue(input.Name, out var system))
            {
                _state.Inputs[vgpr] = input.Name;

                return system;
            }
        }

        foreach (var input in layout)
        {
            if (input.Vgprs.Contains(vgpr) && input.Name == "persp_center")
            {
                // AMD I/J are the weights of vertices 1 and 2: SV_Barycentrics.yz.
                _state.Inputs[vgpr] = "bary";
                var component = "yz"[IndexOf(input.Vgprs, vgpr)];

                return $"asuint(in_bary.{component})";
            }
        }

        if (layout.Any(input => input.Vgprs.Contains(vgpr)))
        {
            throw new UnsupportedProgramException($"implicit input v{vgpr} in ps (barycentric mode outside v_interp)");
        }

        // Outside the hardware input layout: a path-dependent read of an unset VGPR (e.g. loop-carried), undefined.
        return NotPreloaded;
    }

    private static int IndexOf(IReadOnlyList<int> list, int value)
    {
        for (var index = 0; index < list.Count; index++)
        {
            if (list[index] == value)
            {
                return index;
            }
        }

        return -1;
    }

    private string ComputeVector(int vgpr)
    {
        var count = (_state.Config.Compute?.ThreadIdComponentCount ?? 0) + 1;

        if (vgpr < count)
        {
            return $"in_gtid.{Components[vgpr]}";
        }

        return NotPreloaded;
    }

    private string MeshVector(int vgpr)
    {
        if (vgpr == NggVertexIdVgpr)
        {
            // NGG ES vertex id: the flattened dispatch thread id.
            return $"(in_gid.x * {_state.MeshThreads.ToString(CultureInfo.InvariantCulture)}u + in_gi)";
        }

        throw new UnsupportedProgramException($"implicit input v{vgpr} in ms");
    }

    private static string GeometryVector(int vgpr)
    {
        // gfx10 GS VGPRs: v0 = ES vertex offsets 0 | 1 << 16, v1 = 2 | 3 << 16 (dwords into the ESGS ring), v2 = primitive
        // id, v3 = offsets 4 | 5 << 16, v4 = GS instance id. The input vertices sit in the private LDS at GS_IN_BASE.
        switch (vgpr)
        {
            case GsVertexPair01:
                return VertexPair(0, 1);
            case GsVertexPair23:
                return VertexPair(GsVertex2, GsVertex3);
            case GsPrimitiveId:
                return "in_prim";
            case GsVertexPair45:
                return VertexPair(GsVertex4, GsVertex5);
            case GsInstanceId:
                return "0u";
            default:
                return NotPreloaded;
        }
    }

    private static string VertexPair(int first, int second)
    {
        return $"((GS_IN_BASE + {first}u * GS_IN_STRIDE) | ((GS_IN_BASE + {second}u * GS_IN_STRIDE) << 16))";
    }
}
