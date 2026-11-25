// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Containers;
using FiveStar.Shaders.Hardware;
using FiveStar.Shaders.Tests.Toolchain;
using FiveStar.Shaders.Tests.Warp;
using FiveStar.Shaders.Translation;

namespace FiveStar.Shaders.Tests.Cases;

/// <summary>
/// A 128-thread wave64 NGG group (two console waves) whose results need whole console waves: two v_readfirstlanes under
/// EXEC (lanes above thread 40: thread 41 in wave 0, 64 in wave 1; one step in the superstep form), v_readlane of lane 63, a ballot count, and in wave 1
/// only (an SCC branch on the wave index) a region that windows above thread 71 skip with a v_readlane of lane 5 in it.
/// Each thread stores the four values. The superstep form runs it on WARP's 4-lane waves.
/// </summary>
public sealed class MeshSuperstepRunner
{
    public const int Threads = 128;
    public const int Results = 4;

    private const int OutputHeap = 2;
    private const int Wave = 64;
    private const int Size = 8;

    private const string Program = """
        s_mov_b32 m0, 0
        s_sendmsg sendmsg(MSG_GS_ALLOC_REQ)
        v_mul_u32_u24_e32 v2, 3, v5
        s_bfe_u32 s20, s3, 0x40018
        v_cmp_lt_u32_e32 vcc, 40, v5
        s_and_saveexec_b64 s[22:23], vcc
        v_readfirstlane_b32 s30, v5
        v_readfirstlane_b32 s36, v2
        s_mov_b64 exec, s[22:23]
        s_add_u32 s30, s30, s36
        v_readlane_b32 s31, v2, 63
        v_and_b32_e32 v3, 4, v5
        v_cmp_ne_u32_e64 s[34:35], 0, v3
        s_bcnt1_i32_b64 s32, s[34:35]
        s_mov_b32 s33, 0
        s_cmp_eq_u32 s20, 1
        s_cbranch_scc0 .Lshared
        v_cmp_gt_u32_e32 vcc, 0x48, v5
        s_and_saveexec_b64 s[24:25], vcc
        s_cbranch_execz .Ldone
        v_readlane_b32 s33, v2, 5
        .Ldone:
        s_mov_b64 exec, s[24:25]
        .Lshared:
        v_mov_b32 v20, s30
        v_mov_b32 v21, s31
        v_mov_b32 v22, s32
        v_mov_b32 v23, s33
        v_lshlrev_b32_e32 v40, 4, v5
        buffer_store_dword v20, v40, s[8:11], 0 offen
        buffer_store_dword v21, v40, s[8:11], 0 offen offset:4
        buffer_store_dword v22, v40, s[8:11], 0 offen offset:8
        buffer_store_dword v23, v40, s[8:11], 0 offen offset:12
        v_mov_b32 v6, 0
        exp pos0 v6, v6, v6, v6 done
        s_endpgm
        """;

    private readonly WarpFixture _fixture;

    public MeshSuperstepRunner(WarpFixture fixture)
    {
        _fixture = fixture;
    }

    /// <summary>The program in the default form (the dispatch loop with wave emulation).</summary>
    public string Translate()
    {
        return ShaderWorkbench.Translate(Build());
    }

    /// <summary>The default form with fast waves (TranslationOptions.FastWaves): v_readlane of lane 63 still needs the whole wave.</summary>
    public string TranslateFast()
    {
        var options = new TranslationOptions(StructuredControlFlow: false, PinWaveSize: false, EmulateWaveSize: true) { FastWaves = true };

        return HlslTranslator.Translate(Build(), options).Hlsl;
    }

    /// <summary>The superstep form; <paramref name="carry"/> with the liveness carry (TranslationOptions.SuperstepCarry).</summary>
    public string Supersteps(bool carry)
    {
        var options = new TranslationOptions(StructuredControlFlow: false, PinWaveSize: false, EmulateWaveSize: true) { SuperstepCarry = carry };

        return HlslTranslator.SuperstepVariant(Build(), options);
    }

    /// <summary>Runs the superstep form; the mismatches against the console waves.</summary>
    public IReadOnlyList<string> Run(bool carry)
    {
        var name = "ms_supersteps";

        if (carry)
        {
            name += "_carry";
        }

        var mesh = _fixture.Workbench.Compile(name, Supersteps(carry), "ms");
        var pixel = _fixture.Workbench.Compile("ps_supersteps", "float4 main() : SV_Target { return 0; }", "ps");
        var output = new BufferResource(OutputHeap, new uint[Threads * Results], Writable: true);
        var bindings = new Bindings { UserSgprs = ComputeCase.Descriptor(OutputHeap), Buffers = [output] };
        var words = _fixture.Runner.Mesh(new MeshJob(mesh, pixel, Framebuffer.Exact(1, Size, Size), bindings, 1)).Buffers[OutputHeap];
        var mismatches = new List<string>();

        for (var thread = 0; thread < Threads; thread++)
        {
            var expected = Expected(thread);
            var actual = words.AsSpan(thread * Results, Results).ToArray();

            if (!expected.SequenceEqual(actual))
            {
                mismatches.Add($"thread {thread}: expected {string.Join(" ", expected)} actual {string.Join(" ", actual)}");
            }
        }

        return mismatches;
    }

    private ShaderProgram Build()
    {
        var assembled = _fixture.Workbench.Assemble("ms_supersteps", Program, Wave);
        var config = new ShaderConfig { Stage = HardwareStage.Ngg, WaveSize = Wave, UserSgprs = 4, Ngg = new NggConfig(8, 0, 0) };
        var pipeline = new PipelineStateInfo { Stage = "ms", NumThreads = new ThreadGroupSize(Threads, 1, 1), MaxVertices = Threads, MaxPrimitives = Threads };

        return new ShaderProgram(ShaderStage.Mesh, assembled.Instructions, assembled.Code, ShaderSignatures.None, config, pipeline);
    }

    private static uint[] Expected(int thread)
    {
        var wave = thread / Wave;
        uint first = wave == 0 ? 41u * 4u : 64u * 4u;
        uint last = (uint)(((wave * Wave) + Wave - 1) * 3);
        uint fifth = wave == 1 ? (uint)((Wave + 5) * 3) : 0u;

        return [first, last, 32, fifth];
    }
}
