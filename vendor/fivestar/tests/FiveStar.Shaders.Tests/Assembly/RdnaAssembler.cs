// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Disassembly;
using FiveStar.Shaders.Tooling;

namespace FiveStar.Shaders.Tests.Assembly;

/// <summary>
/// Turns a handwritten gfx1013 program (llvm-mc syntax) into code bytes, then disassembles them with the same
/// <see cref="RdnaDecoder"/> the real pipeline uses, so the translator sees exactly what it sees for a dump. The bytes
/// come from the <see cref="AssembledCodeStore"/>; a program it does not hold yet is assembled with llvm-mc and added.
/// </summary>
public sealed class RdnaAssembler
{
    private const int Wave64 = 64;

    private readonly LlvmToolPaths _tools;
    private readonly string _folder;
    private readonly AssembledCodeStore _store;

    public RdnaAssembler(LlvmToolPaths tools, string folder, AssembledCodeStore store)
    {
        _tools = tools;
        _folder = folder;
        _store = store;
    }

    public AssembledProgram Assemble(string name, string source, int waveSize)
    {
        var code = Code(name, source, waveSize);

        return new AssembledProgram(code, new RdnaDecoder().Disassemble(code, waveSize));
    }

    private byte[] Code(string name, string source, int waveSize)
    {
        var path = _store.PathOf(name, source, waveSize, _tools.Cpu);
        var stored = AssembledCodeStore.Read(path);

        if (stored is not null)
        {
            return stored;
        }

        if (!File.Exists(_tools.Assembler))
        {
            throw new InvalidOperationException(
                $"{name} has no assembled code ({Path.GetFileName(path)}): run the tests once with llvm-mc (FIVESTAR_LLVM) to create it");
        }

        var code = RunAssembler(name, source, waveSize);
        _store.Write(path, code);

        return code;
    }

    private byte[] RunAssembler(string name, string source, int waveSize)
    {
        var sourcePath = Path.Combine(_folder, name + ".s");
        var objectPath = Path.Combine(_folder, name + ".o");
        File.WriteAllText(sourcePath, ".text\n" + source + "\n");
        var arguments = new List<string> { "-triple=amdgcn", $"-mcpu={_tools.Cpu}" };

        if (waveSize == Wave64)
        {
            arguments.Add("-mattr=+wavefrontsize64");
        }

        arguments.AddRange(["-filetype=obj", sourcePath, "-o", objectPath]);
        var result = ExternalTool.Run(_tools.Assembler, arguments, timeout: null);

        if (result.ExitCode != 0)
        {
            throw new InvalidOperationException($"llvm-mc rejected {name}: {result.Error.Trim()}");
        }

        return ElfText.Read(File.ReadAllBytes(objectPath));
    }
}
