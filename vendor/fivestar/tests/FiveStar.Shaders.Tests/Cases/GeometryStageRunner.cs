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
/// A handwritten legacy geometry shader (wave64) with its VS copy shader: the input vertices are read from the ESGS LDS
/// ring through the v0 / v1 offsets, records are written at mbcnt * 224 + ring base, then 3 x EMIT + CUT (the input
/// triangle, attribute from v2 = primitive id) and a 4-vertex strip (a 2-triangle quad) + CUT; the copy shader (v0 =
/// record dword offset) exports pos0 / param0. Drawn as 2 input triangles through a pass-through VS, the translated GS
/// and a flat PS, compared with a rasterizer model and the pipeline statistics.
/// </summary>
public sealed class GeometryStageRunner
{
    public const int Size = 8;

    private const string Name = "gs_strips_cut";

    private const string CopyProgram = """
        s_lshr_b32 s2, s3, 24
        s_cmp_lg_u32 s2, 0
        s_cbranch_scc1 .Lend
        s_and_b32 s1, s1, 0xff
        v_lshlrev_b32_e32 v0, 2, v0
        s_load_dword s0, s[0:1], 0xb8
        s_waitcnt lgkmcnt(0)
        v_lshl_add_u32 v12, s0, 8, v0
        ds_read2_b32 v[0:1], v12 offset1:1
        ds_read2_b32 v[2:3], v12 offset0:2 offset1:3
        ds_read2_b32 v[4:5], v12 offset0:4 offset1:5
        ds_read2_b32 v[6:7], v12 offset0:6 offset1:7
        s_waitcnt lgkmcnt(0)
        exp pos0 v0, v1, v2, v3 done
        exp param0 v4, v5, v6, v7
        .Lend:
        s_endpgm
        """;

    // Input triangles in pixel coordinates.
    private static readonly (double X, double Y)[][] Triangles = [[(0, 0), (4, 0), (0, 3)], [(0, 4), (4, 4), (0, 7)]];

    private readonly WarpFixture _fixture;

    public GeometryStageRunner(WarpFixture fixture)
    {
        _fixture = fixture;
    }

    /// <summary>The pass-through VS input layout: float4 position (ATTR0) and float2 texcoord (ATTR1), stride 24.</summary>
    public static IReadOnlyList<VertexElement> InputLayout =>
    [
        new VertexElement("ATTR", 0, Format.R32G32B32A32_Float, 0, 0, PerInstance: false),
        new VertexElement("ATTR", 1, Format.R32G32_Float, 16, 0, PerInstance: false),
    ];

    /// <summary>Draws the strips; <paramref name="direct"/> compiles the geometry shader with the DXIL backend.</summary>
    public IReadOnlyList<string> Run(bool direct = false)
    {
        var hlsl = Translate();
        var (pixelSource, _) = PassThroughPixelShader.Build(hlsl, null, flat: true);
        var job = new GraphicsJob(
            _fixture.Helper("gs_passthrough_vs.hlsl", "vs"),
            _fixture.Workbench.Compile(Name + "_ps", pixelSource, "ps"),
            Framebuffer.Exact(1, Size, Size),
            new Bindings())
        {
            GeometryShader = _fixture.Workbench.Compile(Name, hlsl, "gs", direct),
            InputLayout = InputLayout,
            VertexBuffers = [new VertexBufferData(24, VertexData())],
            VertexCount = 6,
        };

        var result = _fixture.Runner.Graphics(job);
        var image = Reference();
        var mismatches = ImageCheck.Compare(result, Size, Size, (x, y, _) => image[y, x]);
        mismatches.AddRange(ImageCheck.Statistics(result.Statistics, ("GSInvocations", 2), ("GSPrimitives", 6), ("CInvocations", 6)));

        return mismatches;
    }

    /// <summary>Normalized device coordinates of a pixel position on the 8x8 target.</summary>
    public static (double X, double Y) Device(double x, double y)
    {
        return ((x / (Size / 2.0)) - 1, 1 - (y / (Size / 2.0)));
    }

    private static (double S, double T) Texcoord(int primitive, int vertex)
    {
        return ((10.0 * primitive) + vertex + 0.5, 0.25 * vertex);
    }

    private static uint[] VertexData()
    {
        var words = new List<uint>();

        for (var primitive = 0; primitive < Triangles.Length; primitive++)
        {
            for (var vertex = 0; vertex < 3; vertex++)
            {
                var (x, y) = Device(Triangles[primitive][vertex].X, Triangles[primitive][vertex].Y);
                var (s, t) = Texcoord(primitive, vertex);
                words.AddRange([FloatBits.Of(x), FloatBits.Of(y), FloatBits.Of(0.5), FloatBits.Of(1.0), FloatBits.Of(s), FloatBits.Of(t)]);
            }
        }

        return [.. words];
    }

    /// <summary>Strip A: the input triangle with (0, primitive, texcoord of vertex 0); strip B: the quad with (100, primitive, 1, 0).</summary>
    private static Expected[]?[,] Reference()
    {
        var image = new Expected[]?[Size, Size];

        for (var primitive = 0; primitive < Triangles.Length; primitive++)
        {
            var (s, t) = Texcoord(primitive, 0);
            Raster.Fill(image, Triangles[primitive], [0, primitive, FloatBits.Of(s), FloatBits.Of(t)]);
            (double X, double Y)[] quad = [(4, 2 * primitive), (8, 2 * primitive), (4, (2 * primitive) + 2), (8, (2 * primitive) + 2)];
            Expected[] attribute = [100, primitive, 1, 0];
            Raster.Fill(image, [quad[0], quad[1], quad[2]], attribute);
            Raster.Fill(image, [quad[1], quad[2], quad[3]], attribute);
        }

        return image;
    }

    private string Translate()
    {
        var main = _fixture.Workbench.Assemble(Name, GeometryPrograms.Main, 64);
        var copy = _fixture.Workbench.Assemble(Name + "_copy", CopyProgram, 64);
        SignatureElement[] inputs =
        [
            new("SV_Position", 0, "SV_Position", "float", 0, "xyzw", 0xF, 0),
            new("TEXCOORD", 0, string.Empty, "float", 1, "xy", 0x3, 0),
        ];
        SignatureElement[] outputs =
        [
            new("SV_Position", 0, "SV_Position", "float", 0, "xyzw", 0xF, 0),
            new("TEXCOORD", 0, string.Empty, "uint", 1, "xyzw", 0xF, 0),
        ];

        var config = new ShaderConfig { Stage = HardwareStage.Geometry, WaveSize = 64, UserSgprs = 8 };
        var pipeline = new PipelineStateInfo { Stage = "gs", GeometryInputPrimitive = 3, GeometryOutputTopology = 5, MaxVertexCount = 7 };
        var program = new ShaderProgram(ShaderStage.Geometry, main.Instructions, main.Code, new ShaderSignatures(inputs, outputs), config, pipeline)
        {
            CopyShader = copy.Instructions,
        };

        return ShaderWorkbench.Translate(program);
    }
}
