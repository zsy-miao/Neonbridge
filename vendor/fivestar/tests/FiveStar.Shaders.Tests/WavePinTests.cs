// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Containers;
using FiveStar.Shaders.Hardware;
using FiveStar.Shaders.Tests.Toolchain;
using FiveStar.Shaders.Translation;
using Xunit;

namespace FiveStar.Shaders.Tests;

/// <summary>
/// Which compute programs the translator pins with [WaveSize(n)]. A wave32 program with any wave operation is pinned to
/// 32 lanes: in a 64-lane hardware wave (AMD may run an unpinned compute shader so) lanes 32..63 read their mask bits
/// from the half the console has not, and a loop that runs until its mask is empty never ends (a retail game's TAA
/// resolve, the GPU removed after 2 s). A program without wave operations
/// stays unpinned.
/// </summary>
public sealed class WavePinTests : IClassFixture<WarpFixture>
{
    // A loop until no lane is left in s4, the way the console compiler writes one: v_cmpx narrows EXEC, s_andn2 takes
    // the lanes that left out of the loop mask and its SCC says whether one stays.
    private const string MaskLoop = """
        s_mov_b32 s4, exec_lo
        s_branch 1
        s_and_b32 exec_lo, s3, s4
        v_add_nc_u32_e32 v1, 64, v1
        v_cmpx_lt_i32_e32 0x63, v1
        s_andn2_b32 s4, s4, exec_lo
        s_cbranch_scc1 65531
        s_mov_b32 exec_lo, s3
        """;

    private readonly WarpFixture _fixture;

    public WavePinTests(WarpFixture fixture)
    {
        _fixture = fixture;
    }

    [WarpTheory]
    [InlineData("pin_mask_loop", MaskLoop, 32, 64, "[WaveSize(32)]")]
    [InlineData("pin_no_wave_op", "v_add_nc_u32_e32 v1, 64, v1", 32, 64, null)]
    public void A_wave32_compute_program_with_wave_operations_is_pinned_to_32_lanes(string name, string code, int wave, int threads, string? pin)
    {
        var hlsl = Translate(name, code, wave, threads);

        if (pin is null)
        {
            Assert.DoesNotContain("[WaveSize(", hlsl, StringComparison.Ordinal);
        }
        else
        {
            Assert.Contains($"{pin}\n[numthreads(", hlsl, StringComparison.Ordinal);
        }
    }

    private string Translate(string name, string code, int wave, int threads)
    {
        var assembled = _fixture.Workbench.Assemble(name, $"s_mov_b32 s3, exec_lo\n{code}\ns_endpgm", wave);
        var compute = new ComputeConfig
        {
            Rsrc2 = 0,
            ScratchEnable = false,
            GroupSizeEnable = false,
            ThreadIdComponentCount = 1,
            LdsBlocks = 0,
            NumThreads = new ThreadGroupSize(threads, 1, 1),
            GroupIdSgprs = [],
        };
        var config = new ShaderConfig { Stage = HardwareStage.Compute, WaveSize = wave, Compute = compute };
        var program = new ShaderProgram(ShaderStage.Compute, assembled.Instructions, assembled.Code, ShaderSignatures.None, config, null);

        return HlslTranslator.Translate(program, new TranslationOptions(StructuredControlFlow: true, PinWaveSize: true, EmulateWaveSize: true)).Hlsl;
    }
}
