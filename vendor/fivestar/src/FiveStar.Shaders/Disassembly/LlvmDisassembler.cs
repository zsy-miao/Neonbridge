// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using System.Text;
using FiveStar.Shaders.Tooling;

namespace FiveStar.Shaders.Disassembly;

/// <summary>
/// Address-accurate disassembly of RDNA code: the bytes are wrapped in an object file with llvm-mc (<c>.byte</c>
/// directives), then disassembled with llvm-objdump, which resolves branch targets.
/// </summary>
public sealed class LlvmDisassembler : IDisassembler
{
    private const int BytesPerLine = 16;
    private const int Wave64 = 64;

    private readonly LlvmToolPaths _tools;

    public LlvmDisassembler(LlvmToolPaths tools)
    {
        _tools = tools;
    }

    /// <summary>Disassembles code for a wave size; wave64 programs need the wavefrontsize64 attribute to decode right.</summary>
    public IReadOnlyList<Instruction> Disassemble(ReadOnlySpan<byte> code, int waveSize)
    {
        var folder = Directory.CreateTempSubdirectory("fivestar-rdna-");

        try
        {
            var source = Path.Combine(folder.FullName, "c.s");
            var objectFile = Path.Combine(folder.FullName, "c.o");
            File.WriteAllText(source, ByteSource(code));
            Assemble(source, objectFile);

            return ObjdumpParser.Parse(Dump(objectFile, waveSize));
        }
        finally
        {
            folder.Delete(recursive: true);
        }
    }

    private static string ByteSource(ReadOnlySpan<byte> code)
    {
        var text = new StringBuilder(".text\n");

        for (var start = 0; start < code.Length; start += BytesPerLine)
        {
            var line = code.Slice(start, Math.Min(BytesPerLine, code.Length - start));
            text.Append(".byte ");

            for (var index = 0; index < line.Length; index++)
            {
                var separator = index == 0 ? string.Empty : ",";
                text.Append(CultureInfo.InvariantCulture, $"{separator}0x{line[index]:x2}");
            }

            text.Append('\n');
        }

        return text.ToString();
    }

    private void Assemble(string source, string objectFile)
    {
        string[] arguments = ["-triple=amdgcn", $"-mcpu={_tools.Cpu}", "-filetype=obj", source, "-o", objectFile];
        var result = ExternalTool.Run(_tools.Assembler, arguments, timeout: null);

        if (result.ExitCode != 0)
        {
            throw new InvalidOperationException($"llvm-mc failed: {result.Error.Trim()}");
        }
    }

    private string Dump(string objectFile, int waveSize)
    {
        var arguments = new List<string> { "-d", "--triple=amdgcn", $"--mcpu={_tools.Cpu}" };

        if (waveSize == Wave64)
        {
            arguments.Add("--mattr=+wavefrontsize64");
        }

        arguments.Add(objectFile);

        return ExternalTool.Run(_tools.Objdump, arguments, timeout: null).Output;
    }
}
