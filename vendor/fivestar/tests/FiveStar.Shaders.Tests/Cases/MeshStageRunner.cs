// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using System.Text.RegularExpressions;
using FiveStar.Shaders.Containers;
using FiveStar.Shaders.Hardware;
using FiveStar.Shaders.Tests.Reference;
using FiveStar.Shaders.Tests.Toolchain;
using FiveStar.Shaders.Tests.Warp;
using FiveStar.Shaders.Translation;
using FiveStar.Shaders.Translation.Core;

namespace FiveStar.Shaders.Tests.Cases;

/// <summary>
/// A wave64 NGG program through the translator's two-phase mesh path: GS_ALLOC_REQ (m0 = 16 primitives &lt;&lt; 12 | 18
/// vertices), s_barrier, exp pos0 / param0 from lanes below the vertex count, exp prim (three 10-bit indices, bit 31
/// culls) from lanes below the primitive count: 8 horizontal strips of 2 triangles, primitive 5 culled. Drawn natively
/// (DispatchMesh) or through the mesh-to-compute fallback; the image must match a rasterizer model (flat attributes
/// from the provoking vertex) either way. The thread id makes a round trip through ds_bpermute (read from lane id ^ 1,
/// then ^ 1 again), which needs console lane numbers on WARP's 4-lane waves.
/// </summary>
public sealed partial class MeshStageRunner
{
    public const string Native = "ms_ngg_strips";
    public const string FallbackTable = "msfb_ngg_strips";
    public const string FallbackRoot = "msfb_ngg_strips_rootdesc";
    public const string FallbackOverflow = "msfb_overflow_dropped";

    /// <summary>
    /// The program without the ds_bpermute round trip, with an SGPR the vertex region writes but nothing reads after
    /// it, and the culled primitive's index taken with v_readfirstlane inside the primitive region: WARP's 4-lane waves
    /// above the vertex and primitive counts skip those regions while their console wave runs them.
    /// </summary>
    public const string Windows = "ms_ngg_windows";

    /// <summary>The program in the superstep form (HlslTranslator.SuperstepVariant): its barrier and both phases as steps.</summary>
    public const string Supersteps = "ms_ngg_supersteps";

    /// <summary>The superstep form through the mesh-to-compute fallback (GPUs without mesh shaders and narrow waves).</summary>
    public const string SuperstepFallback = "msfb_ngg_supersteps";

    /// <summary>The superstep form with the liveness carry (TranslationOptions.SuperstepCarry).</summary>
    public const string SuperstepCarry = "ms_ngg_supersteps_carry";

    /// <summary>
    /// The program with five loads first (results unused): a buffer of no records through the null page view (s[12:15]),
    /// a good buffer (s[16:19]), a pointer whose page the address map has unmapped (s[20:21]), a buffer through the
    /// record naming no descriptor (s[24:27]) and a 32-bit pointer into the descriptor window (s[28:29]), drawn with the probe (<see cref="RunProbed"/>).
    /// </summary>
    public const string Probe = "ms_ngg_probe";

    /// <summary><see cref="Probe"/> in the superstep form (as the wave32 variant runs on GPUs whose waves are narrower than 64).</summary>
    public const string ProbeSupersteps = "ms_ngg_probe_supersteps";

    private const string ProbeLoads = """
        s_mov_b32 m0, s8
        buffer_load_dword v20, off, s[12:15], 0
        buffer_load_dword v21, off, s[16:19], 0
        s_load_dword s30, s[20:21], 0x0
        buffer_load_dword v22, off, s[24:27], 0
        s_load_dword s31, s[28:29], 0x8
        s_waitcnt vmcnt(0) lgkmcnt(0)
        """;

    private const int ProbeUserSgprs = 22;

    /// <summary>The window address the 32-bit pointer load reads (s[28:29] + 8), and what the window holds at each dword.</summary>
    public const uint WindowAddress = 0x48;
    public const uint WindowFill = 0xD000;
    private const int ProbeBufferWords = 128;
    private const uint UnmappedFill = 0xA000;
    private const uint DataFill = 0xB000;
    private const uint NullRecordFill = 0xC000;
    private const uint RawBuffer = 0x30000000;
    private const uint UnmappedSlot = 8;
    private const uint DataSlot = 3;
    private const uint NullRecordSlot = 5;
    private const uint AddressMapSlot = 7;
    private const uint RingSlot = 1;
    private const uint Records = 256;

    private const string RoundTrip = """
        v_xor_b32_e32 v30, 1, v1
        v_lshlrev_b32_e32 v30, 2, v30
        ds_bpermute_b32 v31, v30, v1
        s_waitcnt lgkmcnt(0)
        v_xor_b32_e32 v1, 1, v31
        """;

    private const string VertexRegion = "s_cbranch_execz .Lprim";
    private const string PrimitiveRegion = "s_cbranch_execz .Lend";

    private const int Vertices = 18;
    private const int Primitives = 16;
    private const int Culled = 5;
    private const double AttributeBase = 100.0;
    private const int Size = 8;
    private const int MaxOutputs = 64;

    private const string Program = """
        s_mov_b32 m0, s8
        s_sendmsg sendmsg(MSG_GS_ALLOC_REQ)
        v_mbcnt_lo_u32_b32 v1, -1, 0
        v_mbcnt_hi_u32_b32 v1, -1, v1
        s_bfe_u32 s20, s3, 0x40018
        s_lshl_b32 s20, s20, 6
        v_add_nc_u32_e32 v1, s20, v1
        v_xor_b32_e32 v30, 1, v1
        v_lshlrev_b32_e32 v30, 2, v30
        ds_bpermute_b32 v31, v30, v1
        s_waitcnt lgkmcnt(0)
        v_xor_b32_e32 v1, 1, v31
        s_barrier
        s_and_b32 s21, s8, 0xfff
        s_mov_b64 s[24:25], exec
        v_cmp_gt_u32_e32 vcc, s21, v1
        s_and_saveexec_b64 s[22:23], vcc
        s_cbranch_execz .Lprim
        v_and_b32_e32 v2, 1, v1
        v_cvt_f32_u32_e32 v2, v2
        v_mul_f32_e32 v2, 2.0, v2
        v_add_f32_e32 v2, -1.0, v2
        v_lshrrev_b32_e32 v3, 1, v1
        v_cvt_f32_u32_e32 v3, v3
        v_mul_f32_e32 v3, 0x3e800000, v3
        v_sub_f32_e32 v3, 1.0, v3
        v_mov_b32 v4, 0
        v_mov_b32 v5, 1.0
        exp pos0 v2, v3, v4, v5 done
        v_cvt_f32_u32_e32 v6, v1
        v_lshrrev_b32_e32 v7, 1, v1
        v_cvt_f32_u32_e32 v7, v7
        v_add_f32_e32 v8, s10, v6
        v_mov_b32 v9, 0.5
        exp param0 v6, v7, v8, v9
        .Lprim:
        s_mov_b64 exec, s[24:25]
        s_lshr_b32 s21, s8, 12
        s_and_b32 s21, s21, 0xfff
        v_cmp_gt_u32_e32 vcc, s21, v1
        s_and_saveexec_b64 s[22:23], vcc
        s_cbranch_execz .Lend
        v_lshrrev_b32_e32 v10, 1, v1
        v_lshlrev_b32_e32 v10, 1, v10
        v_and_b32_e32 v11, 1, v1
        v_add_nc_u32_e32 v12, v10, v11
        v_lshl_add_u32 v13, v11, 1, v10
        v_add_nc_u32_e32 v13, 1, v13
        v_add_nc_u32_e32 v14, 2, v10
        v_lshlrev_b32_e32 v13, 10, v13
        v_lshlrev_b32_e32 v14, 20, v14
        v_or_b32_e32 v15, v12, v13
        v_or_b32_e32 v15, v15, v14
        v_cmp_eq_u32_e64 s[26:27], s9, v1
        v_cndmask_b32_e64 v16, 0, 0x80000000, s[26:27]
        v_or_b32_e32 v15, v15, v16
        exp prim v15, off, off, off done
        .Lend:
        s_endpgm
        """;

    private readonly WarpFixture _fixture;

    public MeshStageRunner(WarpFixture fixture)
    {
        _fixture = fixture;
    }

    public static IReadOnlyList<string> Variants => [Native, FallbackTable, FallbackRoot, FallbackOverflow, Supersteps, SuperstepFallback, SuperstepCarry];

    /// <summary>Draws a variant; <paramref name="direct"/> compiles a native mesh shader with the DXIL backend.</summary>
    public IReadOnlyList<string> Run(string variant, bool direct = false)
    {
        var mesh = Translate(variant);
        var (pixelSource, _) = PassThroughPixelShader.Build(mesh, ["param0"], flat: true);
        var pixel = _fixture.Workbench.Compile(variant + "_ps", pixelSource, "ps");
        var framebuffer = Framebuffer.Exact(1, Size, Size);
        var bindings = new Bindings { UserSgprs = [(Primitives << 12) | Vertices, Culled, FloatBits.Of(AttributeBase)] };
        var image = Reference();

        if (variant is Native or Windows or Supersteps or SuperstepCarry)
        {
            var result = _fixture.Runner.Mesh(new MeshJob(_fixture.Workbench.Compile(variant, mesh, "ms", direct), pixel, framebuffer, bindings, 1));

            return ImageCheck.Compare(result, Size, Size, (x, y, _) => image[y, x]);
        }

        return RunFallback(variant, mesh, pixel, framebuffer, bindings, image);
    }

    private List<string> RunFallback(string variant, string mesh, byte[] pixel, Framebuffer framebuffer, Bindings bindings, Expected[]?[,] image)
    {
        var (compute, vertex) = MeshComputeFallback.Rewrite(mesh);
        var stride = int.Parse(StridePattern().Match(compute).Groups[1].Value, CultureInfo.InvariantCulture);
        var groups = variant == FallbackOverflow ? 3 : 1;
        var rootConstants = variant == FallbackRoot ? 16 : 60;
        var job = new MeshFallbackJob(
            _fixture.Workbench.Compile(variant + "_cs", compute, "cs"),
            _fixture.Workbench.Compile(variant + "_vs", vertex, "vs"),
            pixel,
            framebuffer,
            bindings with { RootConstantCount = rootConstants },
            groups,
            MaxOutputs * stride,
            MaxOutputs * 12)
        {
            RootDescriptors = variant == FallbackRoot,
        };

        if (variant == FallbackOverflow)
        {
            // Buffers for two groups (+4 spare index slots holding stale data whose first triangle, drawn, would repaint
            // pixels (4..7, 0)): the third group must be dropped without drawing anything.
            var stale = new uint[100];
            (stale[96], stale[97], stale[98]) = (0, 1, 3);
            job = job with { VertexBytes = 2 * Vertices * stride, IndexBytes = 100 * 4, StaleIndices = stale };
        }

        var result = _fixture.Runner.MeshFallback(job);
        var mismatches = ImageCheck.Compare(result, Size, Size, (x, y, _) => image[y, x]);
        uint[] arguments = [(uint)(groups * 3 * Primitives), 1, 0, 0, 0, (uint)(groups * Vertices)];

        if (!arguments.SequenceEqual(result.IndirectArguments ?? []))
        {
            mismatches.Add($"DrawIndexed arguments + vertex counter: expected {string.Join(", ", arguments)}, actual {string.Join(", ", result.IndirectArguments ?? [])}");
        }

        return mismatches;
    }

    /// <summary>
    /// Draws <see cref="Probe"/> with the probe code (<see cref="MeshProbe"/>, ring at the start of the writable window):
    /// the image, the records the group wrote and the text of each load site. The address map names the null page view
    /// for every address; the window's views at the slots the V#s name hold real buffers, so every load reads something.
    /// </summary>
    public (IReadOnlyList<string> Mismatches, IReadOnlyList<MeshProbeRecord> Records, IReadOnlyList<string> Sites) RunProbed(string variant, uint id)
    {
        var (mesh, sites) = MeshProbe.Instrument(Translate(variant), id, 0);
        var (pixelSource, _) = PassThroughPixelShader.Build(mesh, ["param0"], flat: true);
        var pixel = _fixture.Workbench.Compile(variant + "_ps", pixelSource, "ps");
        var map = Enumerable.Repeat(UnmappedSlot, ProbeBufferWords).ToArray();
        var bindings = new Bindings
        {
            UserSgprs =
            [
                (Primitives << 12) | Vertices, Culled, FloatBits.Of(AttributeBase), 0,
                UnmappedSlot, 0, 0, RawBuffer,
                DataSlot, 0, Records, RawBuffer,
                0x100, 1, 0, 0,
                NullRecordSlot, 0, Records, RawBuffer,
                WindowAddress - 8, 0,
            ],
            Buffers =
            [
                new BufferResource(0, Filled(WindowFill), Writable: false),
                new BufferResource((int)RingSlot, new uint[ProbeBufferWords], Writable: true),
                new BufferResource((int)DataSlot, Filled(DataFill), Writable: false),
                new BufferResource((int)NullRecordSlot, Filled(NullRecordFill), Writable: false),
                new BufferResource((int)AddressMapSlot, map, Writable: false),
                new BufferResource((int)UnmappedSlot, Filled(UnmappedFill), Writable: false),
            ],
        };

        // As the loader links a mesh program to its pixel shader, after the probe code went in.
        var inputs = PsvSignatureParser.Parse(DxbcContainer.Parse(pixel).Find(PsvSignatureParser.ChunkCode)!.Data.Span).Inputs;
        var linked = StageLinker.Link(mesh, inputs, dropUnread: true);
        var result = _fixture.Runner.Mesh(new MeshJob(_fixture.Workbench.Compile(variant, linked, "ms"), pixel, Framebuffer.Exact(1, Size, Size), bindings, 1));

        var ring = result.Buffers[(int)RingSlot];
        var image = Reference();
        var records = Enumerable.Range(0, (int)ring[0])
            .Select(index => MeshProbeRecord.Read(ring.AsSpan((int)(MeshProbe.RecordsOffset / sizeof(uint)) + (index * MeshProbeRecord.Dwords), MeshProbeRecord.Dwords)))
            .ToList();

        return (ImageCheck.Compare(result, Size, Size, (x, y, _) => image[y, x]), records, sites);
    }

    /// <summary>A probe buffer whose dword i is <paramref name="fill"/> + i.</summary>
    private static uint[] Filled(uint fill)
    {
        return [.. Enumerable.Range(0, ProbeBufferWords).Select(index => fill + (uint)index)];
    }

    /// <summary>The translated program (HLSL) of a variant.</summary>
    public string Translate(string variant)
    {
        var source = Program;

        if (variant == Windows)
        {
            source = Program.Replace(RoundTrip, string.Empty, StringComparison.Ordinal)
                .Replace(VertexRegion, VertexRegion + "\ns_mov_b32 s40, 7", StringComparison.Ordinal)
                .Replace(PrimitiveRegion, PrimitiveRegion + "\nv_mov_b32 v32, s9\nv_readfirstlane_b32 s9, v32", StringComparison.Ordinal);
        }

        var userSgprs = 4;

        if (variant is Probe or ProbeSupersteps)
        {
            source = ProbeLoads + Program["s_mov_b32 m0, s8".Length..];
            userSgprs = ProbeUserSgprs;
        }

        var assembled = _fixture.Workbench.Assemble(variant, source, 64);
        var config = new ShaderConfig { Stage = HardwareStage.Ngg, WaveSize = 64, UserSgprs = userSgprs, Ngg = new NggConfig(8, 0, 0), LdsBytes = 4 };
        var pipeline = new PipelineStateInfo { Stage = "ms", NumThreads = new ThreadGroupSize(64, 1, 1), MaxVertices = MaxOutputs, MaxPrimitives = MaxOutputs };
        var program = new ShaderProgram(ShaderStage.Mesh, assembled.Instructions, assembled.Code, ShaderSignatures.None, config, pipeline);

        if (variant is Supersteps or SuperstepFallback or ProbeSupersteps or SuperstepCarry)
        {
            var options = new TranslationOptions(StructuredControlFlow: false, PinWaveSize: false, EmulateWaveSize: true) { SuperstepCarry = variant == SuperstepCarry };

            return HlslTranslator.SuperstepVariant(program, options);
        }

        return ShaderWorkbench.Translate(program);
    }

    /// <summary>
    /// Vertex j at x = (j odd ? 8 : 0), y = j >> 1 (pixels) with param0 = (j, j >> 1, 100 + j, 0.5); primitive i of strip
    /// k = i >> 1 is (2k, 2k+1, 2k+2) when even, (2k+1, 2k+3, 2k+2) when odd; the culled one draws nothing.
    /// </summary>
    private static Expected[]?[,] Reference()
    {
        var image = new Expected[]?[Size, Size];

        for (var primitive = 0; primitive < Primitives; primitive++)
        {
            var strip = primitive >> 1;
            int[] indices = primitive % 2 == 0 ? [2 * strip, (2 * strip) + 1, (2 * strip) + 2] : [(2 * strip) + 1, (2 * strip) + 3, (2 * strip) + 2];

            if (primitive == Culled || indices.Max() >= Vertices)
            {
                continue;
            }

            var corners = indices.Select(j => ((double)((j & 1) * Size), (double)(j >> 1))).ToArray();
            var provoking = indices[0];
            Expected[] attribute = [FloatBits.Of(provoking), FloatBits.Of(provoking >> 1), FloatBits.Of(AttributeBase + provoking), FloatBits.Of(0.5)];
            Raster.Fill(image, corners, attribute);
        }

        return image;
    }

    [GeneratedRegex(@"vertex record stride (\d+) B")]
    private static partial Regex StridePattern();

    private const string BarrierLoop = """
        s_barrier
        s_mov_b32 s28, s11
        .Lspin:
        s_barrier
        s_sub_u32 s28, s28, 1
        s_cmp_lg_u32 s28, 0
        s_cbranch_scc1 .Lspin
        """;

    // The same count without a barrier: a scalar loop that never reaches a wave operation or a group step.
    private const string SpinLoop = """
        s_barrier
        s_mov_b32 s28, s11
        .Lspin:
        s_sub_u32 s28, s28, 1
        s_cmp_lg_u32 s28, 0
        s_cbranch_scc1 .Lspin
        """;

    /// <summary>
    /// Draws the barrier-loop program with <paramref name="count"/> iterations under <paramref name="caps"/>; with
    /// <paramref name="spin"/> its loop has no barrier.
    /// </summary>
    public IReadOnlyList<string> RunBarrierLoop(bool supersteps, uint count, LoopCaps caps, int? roundsPerStop = null, bool spin = false)
    {
        string name = supersteps ? "ms_ngg_barrier_loop_supersteps" : "ms_ngg_barrier_loop";

        if (spin)
        {
            name += "_spin";
        }

        string mesh = TranslateBarrierLoop(supersteps, caps, roundsPerStop, spin);
        var (pixelSource, _) = PassThroughPixelShader.Build(mesh, ["param0"], flat: true);
        var pixel = _fixture.Workbench.Compile(name + "_ps", pixelSource, "ps");
        var bindings = new Bindings { UserSgprs = [(Primitives << 12) | Vertices, Culled, FloatBits.Of(AttributeBase), count] };

        var result = _fixture.Runner.Mesh(new MeshJob(_fixture.Workbench.Compile(name, mesh, "ms"), pixel, Framebuffer.Exact(1, Size, Size), bindings, 1));
        var image = Reference();

        return ImageCheck.Compare(result, Size, Size, (x, y, _) => image[y, x]);
    }

    /// <summary>The barrier-loop program (<paramref name="spin"/>: without the barrier) translated in the superstep or the two-phase form.</summary>
    public string TranslateBarrierLoop(bool supersteps, LoopCaps caps, int? roundsPerStop = null, bool spin = false)
    {
        var (loop, name) = (BarrierLoop, "ms_ngg_barrier_loop");

        if (spin)
        {
            (loop, name) = (SpinLoop, "ms_ngg_spin_loop");
        }

        var source = Program.Replace("s_barrier\n", loop + "\n", StringComparison.Ordinal);
        var assembled = _fixture.Workbench.Assemble(name, source, 64);
        var config = new ShaderConfig { Stage = HardwareStage.Ngg, WaveSize = 64, UserSgprs = 4, Ngg = new NggConfig(8, 0, 0), LdsBytes = 4 };
        var pipeline = new PipelineStateInfo { Stage = "ms", NumThreads = new ThreadGroupSize(64, 1, 1), MaxVertices = MaxOutputs, MaxPrimitives = MaxOutputs };
        var program = new ShaderProgram(ShaderStage.Mesh, assembled.Instructions, assembled.Code, ShaderSignatures.None, config, pipeline);

        if (supersteps)
        {
            return HlslTranslator.SuperstepVariant(program, new TranslationOptions(StructuredControlFlow: false, PinWaveSize: false, EmulateWaveSize: true) { Loops = caps, RoundsPerStop = roundsPerStop });
        }

        return HlslTranslator.Translate(program, new TranslationOptions(StructuredControlFlow: true, PinWaveSize: false, EmulateWaveSize: true) { Loops = caps }).Hlsl;
    }
}
