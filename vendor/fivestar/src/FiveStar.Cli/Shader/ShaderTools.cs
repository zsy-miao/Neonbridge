// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Compilation;
using FiveStar.Shaders.Disassembly;

namespace FiveStar.Cli.Shader;

/// <summary>
/// Finds the external tools: the LLVM bin folder (llvm-mc, llvm-objdump; only for <c>--llvm-disassembler</c> and
/// <c>decode-check</c>) from <c>--llvm</c>, then FIVESTAR_LLVM, then the default install folder when it exists, then PATH;
/// dxc from <c>--dxc</c>, then FIVESTAR_DXC, then the Windows SDK default when it exists, then PATH.
/// </summary>
internal static class ShaderTools
{
    private const string LlvmVariable = "FIVESTAR_LLVM";
    private const string DxcVariable = "FIVESTAR_DXC";
    private const string PathVariable = "PATH";
    private const string DefaultLlvmFolder = @"D:\Tools\llvm\bin";
    private const string DefaultDxc = @"C:\Program Files (x86)\Windows Kits\10\bin\10.0.26100.0\x64\dxc.exe";

    /// <summary>The switch that disassembles with llvm-mc/llvm-objdump instead of the in-process decoder.</summary>
    public const string LlvmDisassemblerFlag = "llvm-disassembler";

    /// <summary>
    /// The disassembler of a command: the in-process <see cref="RdnaDecoder"/>, or with <c>--llvm-disassembler</c> the
    /// LLVM tools, which are also returned (null for the decoder).
    /// </summary>
    public static (IDisassembler Disassembler, LlvmToolPaths? Llvm) Disassembler(CommandArguments arguments)
    {
        if (!arguments.Flag(LlvmDisassemblerFlag))
        {
            return (new RdnaDecoder(), null);
        }

        var llvm = Llvm(arguments);

        return (new LlvmDisassembler(llvm), llvm);
    }

    public static LlvmToolPaths Llvm(CommandArguments arguments)
    {
        var folder = arguments.Option("llvm") ?? Environment.GetEnvironmentVariable(LlvmVariable);

        if (folder is null && Directory.Exists(DefaultLlvmFolder))
        {
            folder = DefaultLlvmFolder;
        }

        if (folder is null)
        {
            var suffix = OperatingSystem.IsWindows() ? ".exe" : string.Empty;

            return new LlvmToolPaths("llvm-mc" + suffix, "llvm-objdump" + suffix, LlvmToolPaths.DefaultCpu);
        }

        return LlvmToolPaths.InFolder(folder);
    }

    public static string Dxc(CommandArguments arguments)
    {
        var path = arguments.Option("dxc") ?? Environment.GetEnvironmentVariable(DxcVariable);

        if (path is null && File.Exists(DefaultDxc))
        {
            path = DefaultDxc;
        }

        return OnPath(path ?? "dxc");
    }

    public static DxcCompiler Compiler(CommandArguments arguments, bool retry)
    {
        return new DxcCompiler(DxcOptions.Defaults(Dxc(arguments), retry));
    }

    /// <summary>
    /// The compiler <c>--backend</c> names: dxc (hlsl, the default); dxil, the DXIL backend with dxc for what it cannot
    /// lower or dxil.dll rejects; dxil-only, no dxc at all.
    /// </summary>
    public static IShaderCompiler Backend(CommandArguments arguments, bool retry)
    {
        var dxc = Compiler(arguments, retry);

        return arguments.Option("backend") switch
        {
            null or "hlsl" => dxc,
            "dxil" => new DxilBackendCompiler(dxc.Options, dxc, new InProcessBackend(DxilBackendCompiler.ValidatorBeside(dxc.Options.Path))),
            "dxil-only" => new DxilBackendCompiler(dxc.Options, null, new InProcessBackend(null)),
            var other => throw new ArgumentException($"--backend {other}: expected hlsl, dxil or dxil-only"),
        };
    }

    /// <summary>The full path of a bare tool name found in a PATH folder; other names unchanged.</summary>
    private static string OnPath(string tool)
    {
        if (Path.IsPathRooted(tool) || tool.Contains(Path.DirectorySeparatorChar, StringComparison.Ordinal))
        {
            return tool;
        }

        var suffix = OperatingSystem.IsWindows() && !Path.HasExtension(tool) ? ".exe" : string.Empty;
        var folders = (Environment.GetEnvironmentVariable(PathVariable) ?? string.Empty).Split(Path.PathSeparator, StringSplitOptions.RemoveEmptyEntries);
        var found = folders.Select(folder => Path.Combine(folder, tool + suffix)).FirstOrDefault(File.Exists);

        return found ?? tool;
    }
}
