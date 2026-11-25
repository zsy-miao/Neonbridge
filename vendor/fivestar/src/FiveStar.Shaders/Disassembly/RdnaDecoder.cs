// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;
using FiveStar.Shaders.Disassembly.Rdna;

namespace FiveStar.Shaders.Disassembly;

/// <summary>
/// Decodes RDNA 2 machine code in-process. The text of every instruction is exactly what <see cref="LlvmDisassembler"/>
/// returns for the same bytes (llvm-objdump for gfx1013, branch targets as <c>@T0x&lt;target&gt;</c>), so the
/// translator reads both the same way; <c>fivestar shader decode-check</c> compares the two on a corpus.
/// </summary>
public sealed class RdnaDecoder : IDisassembler
{
    private const int BytesPerWord = 4;
    private const int Wave64 = 64;
    private const int MinimumSkippedZeros = 8;

    public IReadOnlyList<Instruction> Disassemble(ReadOnlySpan<byte> code, int waveSize)
    {
        var words = new uint[code.Length / BytesPerWord];

        for (var index = 0; index < words.Length; index++)
        {
            words[index] = BinaryPrimitives.ReadUInt32LittleEndian(code[(index * BytesPerWord)..]);
        }

        var program = new CodeWords(words, waveSize == Wave64);
        var instructions = new List<Instruction>();
        var at = 0;

        while (at < words.Length)
        {
            var zeros = SkippedZeroWords(code[(at * BytesPerWord)..]);

            if (zeros > 0)
            {
                at += zeros;
                continue;
            }

            var decoded = InstructionDecoder.Decode(program, at);

            if (!decoded.Text.Contains(OperandText.Unaligned, StringComparison.Ordinal))
            {
                instructions.Add(new Instruction(at * BytesPerWord, decoded.Words * BytesPerWord, decoded.Text));
            }

            at += decoded.Words;
        }

        return instructions;
    }

    /// <summary>
    /// llvm-objdump leaves out a run of at least eight zero bytes at an instruction start (it prints "..."), in whole
    /// dwords; the run may end inside the next dword.
    /// </summary>
    private static int SkippedZeroWords(ReadOnlySpan<byte> rest)
    {
        var zeros = rest.IndexOfAnyExcept((byte)0);

        if (zeros < 0)
        {
            zeros = rest.Length;
        }

        if (zeros < MinimumSkippedZeros)
        {
            return 0;
        }

        return zeros / BytesPerWord;
    }
}
