// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Compilation;
using FiveStar.Shaders.Disassembly;
using FiveStar.Shaders.Tests.Reference;
using FiveStar.Shaders.Tests.Toolchain;
using FiveStar.Shaders.Tests.Warp;

namespace FiveStar.Shaders.Tests.Cases;

/// <summary>
/// A real GS_VoxelizeGeneric program from the user's dump, end to end (its own disassembly, copy shader, DXBC
/// signatures, PSV0 and FSSM header): 3 synthetic triangles, one per dominant axis; the constant buffer is reached
/// through user SGPR s16 -> descriptor table in the GPU-memory window (V# at +0x920 -> heap slot 5, scale 1.0 at
/// 0x40..0x4c). Checked against a model of the program: dominant-axis projection, axis id, constant attribute,
/// interpolated texcoord and normal for pixels at least 0.05 px inside a triangle, clear pixels outside all of them,
/// and the pipeline statistics.
/// </summary>
public sealed class RealGeometryRunner
{
    private const int Size = GeometryStageRunner.Size;
    private const double Inside = 0.05;
    private const int ConstantsHeap = 5;

    // World-space triangles with a clear dominant axis each (z, x, y), landing in separate screen areas.
    private static readonly double[][][] Triangles =
    [
        [[-1.0, 1.0, 0.2], [0.0, 1.0, 0.2], [-1.0, 0.25, 0.2]],
        [[0.3, -0.25, 0.0], [0.3, -0.25, 1.0], [0.3, -1.0, 0.0]],
        [[-1.0, -0.5, -0.25], [0.0, -0.5, -0.25], [-1.0, -0.5, -1.0]],
    ];

    private readonly WarpFixture _fixture;

    public RealGeometryRunner(WarpFixture fixture)
    {
        _fixture = fixture;
    }

    /// <summary>Runs the program whose file name in <paramref name="folder"/> contains GS_VoxelizeGeneric_&lt;hash&gt; (<paramref name="direct"/>: compiled by the DXIL backend).</summary>
    public IReadOnlyList<string> Run(string folder, string hash, bool direct = false)
    {
        var path = Directory.GetFiles(folder, $"*GS_VoxelizeGeneric_{hash}*.fssm").FirstOrDefault();

        if (path is null)
        {
            return [$"no *GS_VoxelizeGeneric_{hash}*.fssm in {folder}"];
        }

        var loaded = new ProgramLoader(new RdnaDecoder()).Load(path, null);
        var program = loaded.Program ?? throw new InvalidOperationException($"{loaded.Status}: {loaded.Reason}");
        var hlsl = ShaderWorkbench.Translate(program);
        var (pixelSource, semantics) = PassThroughPixelShader.Build(hlsl, null, flat: false);
        var job = new GraphicsJob(
            _fixture.Helper("gs_passthrough_vs.hlsl", "vs"),
            _fixture.Workbench.Compile($"gs_real_{hash}_ps", pixelSource, "ps"),
            Framebuffer.Exact(semantics.Count, Size, Size),
            Bindings())
        {
            GeometryShader = _fixture.Workbench.Compile($"gs_real_{hash}", hlsl, "gs", direct),
            InputLayout = GeometryStageRunner.InputLayout,
            VertexBuffers = [new VertexBufferData(24, VertexData())],
            VertexCount = 9,
        };

        var result = _fixture.Runner.Graphics(job);
        var image = Reference();
        var mismatches = ImageCheck.Compare(result, Size, Size, (x, y, target) => Pixel(image[y, x], semantics[target]));
        mismatches.AddRange(ImageCheck.Statistics(result.Statistics, ("GSInvocations", 3), ("GSPrimitives", 3), ("CInvocations", 3)));

        return mismatches;
    }

    private static Bindings Bindings()
    {
        var window = new uint[0x260];
        uint[] descriptor = [ConstantsHeap, 0, 0x100, 0];
        descriptor.CopyTo(window, 0x920 / 4);
        var constants = new uint[64];
        Array.Fill(constants, FloatBits.Of(1.0), 16, 4);

        return new Bindings
        {
            UserSgprs = new uint[16],
            Buffers = [new BufferResource(0, window, Writable: false), new BufferResource(ConstantsHeap, constants, Writable: false)],
        };
    }

    private static (double S, double T) Texcoord(int primitive, int vertex)
    {
        return (primitive + (0.125 * vertex) + 0.0625, 2.0 - (0.5 * vertex));
    }

    private static uint[] VertexData()
    {
        var words = new List<uint>();

        for (var primitive = 0; primitive < Triangles.Length; primitive++)
        {
            for (var vertex = 0; vertex < 3; vertex++)
            {
                var (s, t) = Texcoord(primitive, vertex);
                words.AddRange([.. Triangles[primitive][vertex].Select(FloatBits.Of), FloatBits.Of(1.0), FloatBits.Of(s), FloatBits.Of(t)]);
            }
        }

        return [.. words];
    }

    /// <summary>The expected words of one target: null for clear, don't-care near an edge, else by output semantic.</summary>
    private static Expected[]? Pixel(VoxelPixel? pixel, string semantic)
    {
        if (pixel is null)
        {
            return null;
        }

        if (pixel.NearEdge)
        {
            return [Expected.Any, Expected.Any, Expected.Any, Expected.Any];
        }

        switch (semantic)
        {
            case "TEXCOORD0":
                return [pixel.Axis, 0, 0, 0];
            case "TEXCOORD1":
                return [Expected.Near(-1, 1e-5), Expected.Near(-1, 1e-5), Expected.Near(1, 1e-5), Expected.Near(1, 1e-5)];
            case "TEXCOORD2":
                return [Expected.Near(pixel.S, 1e-4), Expected.Near(pixel.T, 1e-4), 0, 0];
            case "TEXCOORD3":
                return [Expected.Near(pixel.Normal[0], 1e-3), Expected.Near(pixel.Normal[1], 1e-3), Expected.Near(pixel.Normal[2], 1e-3), 0];
            default:
                return [Expected.Any, Expected.Any, Expected.Any, Expected.Any];
        }
    }

    private static VoxelPixel?[,] Reference()
    {
        var image = new VoxelPixel?[Size, Size];

        for (var primitive = 0; primitive < Triangles.Length; primitive++)
        {
            VoxelModel.Paint(image, Triangles[primitive], primitive, Texcoord, Inside);
        }

        return image;
    }
}
