// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Tooling;
using FiveStar.Shaders.Translation;

namespace FiveStar.Shaders.Compilation;

/// <summary>
/// Compiles translated HLSL with dxc: shader model 6.7 for every stage (programmable texel offsets, helper-lane wave
/// operations), HLSL 2021, entry point main, the source's folder (with the runtime headers) as include path. Each compile
/// is one dxc.exe process, killed at the time limit.
/// </summary>
public sealed class DxcCompiler : IShaderCompiler
{
    private const string ShaderModel = "_6_7";
    private const string CompilerFile = "dxcompiler.dll";
    private const string ValidatorFile = "dxil.dll";
    private const int ErrorTextLength = 200;

    /// <summary>The arguments besides the profile, the include folder and -Od, for compiler identities.</summary>
    internal const string FixedArguments = "-E main -HV 2021; -Gis for vs, ms and gs";

    /// <summary>
    /// IEEE strictness (no fast-math reassociation or contraction) for the stages that write SV_Position: two programs that
    /// compute the same position (a depth prepass and a pass that tests EQUAL) must round it the same, which dxc's default
    /// fast math does not promise (SharpEmu marks every operation feeding the position precise; facts only). The DXIL
    /// backend writes no fast-math flags at all.
    /// </summary>
    private const string StrictArgument = "-Gis";

    private static readonly string[] PositionStages = ["vs", "ms", "gs"];

    private readonly DxcOptions _options;
    private readonly Lazy<string> _identity;

    public DxcCompiler(DxcOptions options)
    {
        _options = options;
        _identity = new Lazy<string>(ComputeIdentity);
    }

    public DxcOptions Options => _options;

    public string Identity => _identity.Value;

    public CompileOutcome Compile(CompileRequest request, TimeSpan timeout)
    {
        var arguments = Arguments(request);
        arguments.InsertRange(arguments.Count - 1, ["-nologo", "-Fo", request.DxilPath]);
        var result = ExternalTool.Run(_options.Path, arguments, timeout);

        if (result.TimedOut)
        {
            return CompileOutcome.Timeout;
        }

        if (result.ExitCode != 0)
        {
            return CompileOutcome.Failure(result.Error);
        }

        return CompileOutcome.Success with { Error = result.Error };
    }

    /// <summary>The first error line of a failed compile (or the start of its messages).</summary>
    public static string FirstError(string error)
    {
        var text = error.Replace("\r\n", "\n", StringComparison.Ordinal);
        var line = text.Split('\n').FirstOrDefault(candidate => candidate.Contains("error", StringComparison.Ordinal));

        if (line is not null)
        {
            return line;
        }

        return text[..Math.Min(ErrorTextLength, text.Length)];
    }

    /// <summary>The dxc arguments of a request, the source file last.</summary>
    internal static List<string> Arguments(CompileRequest request)
    {
        var folder = Path.GetDirectoryName(Path.GetFullPath(request.HlslPath)) ?? ".";
        var arguments = new List<string> { "-T", request.Profile + ShaderModel, "-E", "main", "-HV", "2021", "-I", folder };

        if (request.DisableOptimizations)
        {
            arguments.Add("-Od");
        }

        if (PositionStages.Contains(request.Profile))
        {
            arguments.Add(StrictArgument);
        }

        arguments.Add(request.HlslPath);

        return arguments;
    }

    /// <summary>The dxc files, the fixed arguments and the runtime headers the sources include.</summary>
    private string ComputeIdentity()
    {
        var folder = Path.GetDirectoryName(_options.Path) ?? string.Empty;
        string[] files = [_options.Path, Path.Combine(folder, CompilerFile), Path.Combine(folder, ValidatorFile)];
        string[] arguments = [ShaderModel, FixedArguments];

        return ContentHash.OfText([ContentHash.OfFiles(files), .. arguments, .. RuntimeIncludes.Names.Select(RuntimeIncludes.Read)]);
    }
}
