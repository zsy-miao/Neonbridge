// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Compilation;
using FiveStar.Shaders.Dxil;
using FiveStar.Shaders.Tests.Assembly;
using FiveStar.Shaders.Tooling;
using FiveStar.Shaders.Translation;

namespace FiveStar.Shaders.Tests.Toolchain;

/// <summary>
/// The build steps of a translator test in one work folder: the assembled program, translation, dxc (shader model 6.7,
/// HLSL 2021, the runtime headers next to the source) and the helper shaders embedded in this assembly.
/// </summary>
public sealed class ShaderWorkbench
{
    private static readonly TimeSpan CompileTimeout = TimeSpan.FromMinutes(2);
    private static readonly TranslationOptions Options = new(StructuredControlFlow: true, PinWaveSize: false, EmulateWaveSize: true);

    private readonly RdnaAssembler _assembler;
    private readonly DxcCompiler _compiler;
    private readonly string _dxc;
    private readonly string _folder;

    public ShaderWorkbench(ShaderToolchain tools, string folder)
    {
        _folder = folder;
        _assembler = new RdnaAssembler(tools.Llvm, folder, AssembledCodeStore.InRepository());
        _compiler = new DxcCompiler(DxcOptions.Defaults(tools.Dxc, retry: false));
        _dxc = tools.Dxc;
        RuntimeIncludes.WriteTo(folder);
    }

    /// <summary>The lines every compiled source starts with (<see cref="Warp.UserDataBindings.Header"/>).</summary>
    public string Header { get; init; } = string.Empty;

    public AssembledProgram Assemble(string name, string source, int waveSize)
    {
        return _assembler.Assemble(name, source, waveSize);
    }

    /// <summary>The form <c>fivestar shader translate</c> emits: structured control flow, wave sizes emulated, not pinned.</summary>
    public static string Translate(ShaderProgram program)
    {
        return HlslTranslator.Translate(program, Options).Hlsl;
    }

    /// <summary>Compiles HLSL with the profile prefix (ps, vs, cs, ms, gs) to DXIL; dxc errors become an exception.</summary>
    public byte[] Compile(string name, string hlsl, string profile)
    {
        var path = Path.Combine(_folder, name + ".hlsl");
        hlsl = Header + hlsl;
        File.WriteAllText(path, hlsl);
        var outcome = _compiler.Compile(new CompileRequest(path, hlsl, profile, DisableOptimizations: false), CompileTimeout);

        if (!outcome.Succeeded)
        {
            throw new InvalidOperationException($"dxc rejected {name} ({profile}): {DxcCompiler.FirstError(outcome.Error)}\n{path}");
        }

        return File.ReadAllBytes(Path.ChangeExtension(path, ".dxil"));
    }

    /// <summary>Compiles with the DXIL backend when <paramref name="direct"/> is set, else with dxc.</summary>
    public byte[] Compile(string name, string hlsl, string profile, bool direct)
    {
        if (direct)
        {
            return DxilBackend.Compile(Header + hlsl, profile);
        }

        return Compile(name, hlsl, profile);
    }

    /// <summary>
    /// Compiles with IEEE strictness (<c>-Gis</c>): no fast-math reassociation or contraction, so the result depends on
    /// the operations alone, not on how the optimizer sees the code around them.
    /// </summary>
    public byte[] CompileStrict(string name, string hlsl, string profile)
    {
        var path = Path.Combine(_folder, name + ".strict.hlsl");
        File.WriteAllText(path, Header + hlsl);
        var dxil = Path.ChangeExtension(path, ".dxil");
        string[] arguments = ["-T", profile + "_6_7", "-E", "main", "-HV", "2021", "-Gis", "-I", _folder, "-Fo", dxil, path];
        var result = ExternalTool.Run(_dxc, arguments, CompileTimeout);

        if (result.ExitCode != 0)
        {
            throw new InvalidOperationException($"dxc -Gis rejected {name} ({profile}): {DxcCompiler.FirstError(result.Error)}");
        }

        return File.ReadAllBytes(dxil);
    }

    /// <summary>Compiles a helper shader embedded from the Shaders folder.</summary>
    public byte[] CompileHelper(string fileName, string profile)
    {
        using var stream = typeof(ShaderWorkbench).Assembly.GetManifestResourceStream(fileName)
            ?? throw new FileNotFoundException("helper shader not embedded", fileName);
        using var reader = new StreamReader(stream);

        return Compile(Path.GetFileNameWithoutExtension(fileName), reader.ReadToEnd(), profile);
    }
}
