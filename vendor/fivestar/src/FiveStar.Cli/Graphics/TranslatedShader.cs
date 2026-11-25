// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Compilation;
using FiveStar.Shaders.Disassembly;
using FiveStar.Shaders.Translation;

namespace FiveStar.Cli.Graphics;

/// <summary>A program translated to HLSL and compiled to DXIL.</summary>
public sealed record TranslatedShader(string Name, string Hlsl, byte[] Dxil)
{
    /// <summary>
    /// Translates and compiles <c>&lt;program folder&gt;/&lt;name&gt;.fssm</c> for each name into
    /// <paramref name="workFolder"/>, as <c>fivestar shader translate</c> does.
    /// </summary>
    public static IReadOnlyList<TranslatedShader> Translate(
        IReadOnlyList<string> names, string programFolder, string workFolder, IDisassembler disassembler, string dxc)
    {
        var compiler = new DxcCompiler(DxcOptions.Defaults(dxc, retry: false));
        var loader = new ProgramLoader(disassembler);
        var paths = names.Select(name => Path.Combine(programFolder, name + ".fssm")).ToList();
        var options = new CorpusOptions(workFolder, null, names.Count, null)
        {
            Translation = new TranslationOptions(StructuredControlFlow: true, PinWaveSize: true, EmulateWaveSize: true),
        };
        var results = new CorpusTranslator(loader, compiler, null, null, null).Run(paths, options);
        var failed = results.FirstOrDefault(result => result.Status != ProgramStatus.Ok);

        if (failed is not null)
        {
            throw new InvalidOperationException($"{failed.Name}: {failed.Status} {failed.Reason}");
        }

        return names.Select(name => Load(workFolder, name)).ToList();
    }

    private static TranslatedShader Load(string folder, string name)
    {
        var stem = Path.Combine(folder, name);

        return new TranslatedShader(name, File.ReadAllText(stem + ".hlsl"), File.ReadAllBytes(stem + ".dxil"));
    }
}
