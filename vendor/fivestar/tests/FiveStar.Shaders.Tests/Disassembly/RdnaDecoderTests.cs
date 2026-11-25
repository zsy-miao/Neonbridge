// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;
using FiveStar.Shaders.Disassembly;
using FiveStar.Shaders.Tests.Toolchain;
using Xunit;

namespace FiveStar.Shaders.Tests.Disassembly;

/// <summary>The in-process decoder lists code exactly as the llvm-objdump pipeline does.</summary>
public sealed class RdnaDecoderTests
{
    private const int Wave32 = 32;
    private const int Wave64 = 64;

    /// <summary>
    /// llvm-objdump's listing of these dwords: a literal, modifiers, a branch target, an instruction it lists with an
    /// alignment warning (the listing parser drops it), a skipped run of zeros and a dword that is no instruction.
    /// </summary>
    [Fact]
    public void Lists_code_the_way_llvm_objdump_does()
    {
        uint[] words = [0xBFA00003, 0x4A2C60FF, 0x00000060, 0xD54B0302, 0x23C21702, 0xBF850001, 0xBEB504F3, 0xBF810000, 0, 0, 0x04030201];
        var code = new byte[words.Length * sizeof(uint)];

        for (var index = 0; index < words.Length; index++)
        {
            BinaryPrimitives.WriteUInt32LittleEndian(code.AsSpan(index * sizeof(uint)), words[index]);
        }

        Instruction[] expected =
        [
            new(0x00, 4, "s_inst_prefetch 0x3"),
            new(0x04, 8, "v_add_nc_u32_e32 v22, 0x60, v48"),
            new(0x0C, 8, "v_fma_f32 v2, -|v2|, |v11|, 0.5"),
            new(0x14, 4, "s_cbranch_scc1 1 @T0x1c"),
            new(0x1C, 4, "s_endpgm"),
            new(0x28, 4, ".long 0x04030201"),
        ];

        Assert.Equal(expected, new RdnaDecoder().Disassemble(code, Wave32));
    }

    [Theory]
    [InlineData(Wave32, "v_cmp_eq_f32_e64 s0, s22, 0")]
    [InlineData(Wave64, "v_cmp_eq_f32_e64 s[0:1], s22, 0")]
    public void Lane_masks_follow_the_wave_size(int waveSize, string text)
    {
        byte[] code = [0x00, 0x00, 0x02, 0xD4, 0x16, 0x00, 0x01, 0x00];

        Assert.Equal(text, Assert.Single(new RdnaDecoder().Disassemble(code, waveSize)).Text);
    }

    /// <summary>
    /// llvm-mc 23 writes the inline constant 0 into the source a VOP3 compare does not read (d4c40028 02020084, where
    /// older releases wrote 0): llvm-objdump lists it as the same compare.
    /// </summary>
    [Fact]
    public void An_unread_vop3_source_may_be_the_inline_zero()
    {
        byte[] code = [0x28, 0x00, 0xC4, 0xD4, 0x84, 0x00, 0x02, 0x02];

        Assert.Equal("v_cmp_gt_u32_e64 s40, 4, v0", Assert.Single(new RdnaDecoder().Disassemble(code, Wave32)).Text);
    }

    /// <summary>The smallest programs of each stage of the dump decode exactly as with llvm-mc/llvm-objdump.</summary>
    [LlvmProgramTheory]
    [InlineData(8)]
    public void Real_programs_decode_as_with_llvm(int perStage)
    {
        var programs = new DirectoryInfo(RealProgramTheoryAttribute.Folder!)
            .GetFiles("*.fssm")
            .GroupBy(file => file.Name.Split('_')[2], StringComparer.Ordinal)
            .SelectMany(stage => stage.OrderBy(file => file.Length).ThenBy(file => file.Name, StringComparer.Ordinal).Take(perStage));
        var check = new DecoderCheck(new LlvmDisassembler(ShaderToolchain.Find().Llvm), new RdnaDecoder());
        var mismatches = programs.SelectMany(program => check.Check(program.FullName).Mismatches).ToList();

        Assert.Empty(mismatches);
    }
}
