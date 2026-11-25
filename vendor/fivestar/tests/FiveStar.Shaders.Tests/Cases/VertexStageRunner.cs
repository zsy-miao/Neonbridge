// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Containers;
using FiveStar.Shaders.Hardware;
using FiveStar.Shaders.Tests.Reference;
using FiveStar.Shaders.Tests.Toolchain;
using FiveStar.Shaders.Tests.Warp;
using FiveStar.Shaders.Translation;
using Vortice.DXGI;

namespace FiveStar.Shaders.Tests.Cases;

/// <summary>
/// A vertex shader with fetch-shader inputs: the fetch call (s_swappc at entry) and the fetch table give attribute 0
/// (per-vertex float4) in v4..v7 and attribute 1 (per-instance, 2 components) in v8..v9; v0 = SV_VertexID,
/// v3 = SV_InstanceID. A point list of 8 vertices x 2 instances, one pixel per vertex, params exported as uint.
/// </summary>
public sealed class VertexStageRunner
{
    private const string Name = "vs_fetch_points";
    private const int Width = 8;
    private const int Height = 2;
    private const uint UserValue = 1000;

    private const string Program = """
        s_swappc_b64 s[4:5], s[4:5]
        v_cvt_f32_u32_e32 v10, v0
        v_add_f32_e32 v10, 0.5, v10
        v_mul_f32_e32 v10, 0x3e800000, v10
        v_add_f32_e32 v10, -1.0, v10
        v_cvt_f32_u32_e32 v11, v3
        v_add_f32_e32 v11, 0.5, v11
        v_sub_f32_e32 v11, 1.0, v11
        v_mov_b32 v12, 0
        v_mov_b32 v13, 1.0
        exp pos0 v10, v11, v12, v13 done
        exp param0 v4, v5, v6, v7
        v_add_nc_u32_e32 v14, s8, v0
        exp param1 v8, v9, v14, v3
        s_endpgm
        """;

    // Two instances x 2 dwords, denormals included.
    private static readonly uint[] InstanceData = [0x00000005, 0x7F7FFFFF, 0x00010002, 0x80000001];

    private readonly WarpFixture _fixture;

    public VertexStageRunner(WarpFixture fixture)
    {
        _fixture = fixture;
    }

    /// <summary>Draws the points; <paramref name="direct"/> compiles the vertex shader with the DXIL backend.</summary>
    public IReadOnlyList<string> Run(bool direct = false)
    {
        var vertexData = Enumerable.Range(0, Width).SelectMany(j => new[] { FloatBits.Of(1.5 * j), FloatBits.Of(-2.0 - j), 0x3F800000 + (uint)j, 0x12345678 + (uint)j }).ToArray();
        var hlsl = Translate();
        var (pixelSource, _) = PassThroughPixelShader.Build(hlsl, ["p0_xyzw", "p1_xyzw"], flat: true);
        var vertex = _fixture.Workbench.Compile(Name, hlsl, "vs", direct);
        var job = new GraphicsJob(
            vertex,
            _fixture.Workbench.Compile(Name + "_ps", pixelSource, "ps"),
            Framebuffer.Exact(2, Width, Height),
            new Bindings { UserSgprs = [0, 0, 0, 0, 0, 0, 0, 0, UserValue] })
        {
            InputLayout =
            [
                new VertexElement("ATTR", 0, Format.R32G32B32A32_Float, 0, 0, PerInstance: false),
                new VertexElement("ATTR", 1, Format.R32G32_Float, 0, 1, PerInstance: true),
            ],
            VertexBuffers = [new VertexBufferData(16, vertexData), new VertexBufferData(8, InstanceData)],
            Points = true,
            VertexCount = Width,
            InstanceCount = Height,
        };
        var result = _fixture.Runner.Graphics(job);

        return ImageCheck.Compare(result, Width, Height, (x, y, target) => Wanted(vertexData, x, y, target));
    }

    private static Expected[] Wanted(uint[] vertexData, int x, int y, int target)
    {
        if (target == 0)
        {
            return [.. vertexData.Skip(4 * x).Take(4).Select(Expected.Exact)];
        }

        return [InstanceData[2 * y], InstanceData[(2 * y) + 1], UserValue + (uint)x, (uint)y];
    }

    private string Translate()
    {
        var assembled = _fixture.Workbench.Assemble(Name, Program, 64);
        var vertex = new VertexConfig
        {
            VgprComponentCount = 3,
            ParamExports = 2,
            FetchShader = true,
            FetchPcSgpr = 4,
            FetchInputs = [new FetchInput(1, 0, 15, 4, 4), new FetchInput(1, 1, 3, 8, 2)],
        };
        var config = new ShaderConfig { Stage = HardwareStage.Vertex, WaveSize = 64, UserSgprs = 12, Vertex = vertex };
        SignatureElement[] outputs =
        [
            new("SV_Position", 0, "SV_Position", "float", 0, "xyzw", 0xF, 0),
            new("TEXCOORD", 0, string.Empty, "uint", 1, "xyzw", 0xF, 0),
            new("TEXCOORD", 1, string.Empty, "uint", 2, "xyzw", 0xF, 0),
        ];
        var program = new ShaderProgram(ShaderStage.Vertex, assembled.Instructions, assembled.Code, new ShaderSignatures([], outputs), config, null);

        return ShaderWorkbench.Translate(program);
    }
}
