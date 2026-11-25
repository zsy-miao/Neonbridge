// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Hardware;

namespace FiveStar.Shaders.Disassembly;

/// <summary>
/// Disassembles the code of an FSSM program with a reference and a candidate disassembler and finds where the listings
/// differ: the main code at the program's wave size and, for a geometry program, the copy shader at wave64, as
/// <see cref="Compilation.ProgramLoader"/> loads them.
/// </summary>
public sealed class DecoderCheck
{
    private const int Wave64 = 64;

    private readonly IDisassembler _reference;
    private readonly IDisassembler _candidate;

    public DecoderCheck(IDisassembler reference, IDisassembler candidate)
    {
        _reference = reference;
        _candidate = candidate;
    }

    /// <summary>The comparison of one program; a program whose header does not decode has no blocks.</summary>
    public DecodeCheckResult Check(string programPath)
    {
        var bytes = File.ReadAllBytes(programPath);
        ShaderConfig config;

        try
        {
            config = FssmHeader.Decode(bytes);
        }
        catch (Exception exception) when (exception is InvalidDataException or ArgumentOutOfRangeException)
        {
            return new DecodeCheckResult(0, 0, []);
        }

        var name = Path.GetFileNameWithoutExtension(programPath);
        var blocks = new List<(string Block, byte[] Code, int Wave)> { ("main", FssmHeader.ProgramCode(bytes), config.WaveSize ?? Wave64) };

        if (config.Stage == HardwareStage.Geometry)
        {
            blocks.Add(("copy", FssmHeader.CopyShaderCode(bytes), Wave64));
        }

        var instructions = 0;
        var mismatches = new List<DecodeMismatch>();

        foreach (var (block, code, wave) in blocks)
        {
            var expected = _reference.Disassemble(code, wave);
            var mismatch = FirstDifference(expected, _candidate.Disassemble(code, wave));
            instructions += expected.Count;

            if (mismatch is not null)
            {
                mismatches.Add(new DecodeMismatch(name, block, mismatch.Value.Expected, mismatch.Value.Actual));
            }
        }

        return new DecodeCheckResult(blocks.Count, instructions, mismatches);
    }

    private static (Instruction? Expected, Instruction? Actual)? FirstDifference(IReadOnlyList<Instruction> expected, IReadOnlyList<Instruction> actual)
    {
        for (var index = 0; index < Math.Max(expected.Count, actual.Count); index++)
        {
            var left = expected.ElementAtOrDefault(index);
            var right = actual.ElementAtOrDefault(index);

            if (left != right)
            {
                return (left, right);
            }
        }

        return null;
    }
}
