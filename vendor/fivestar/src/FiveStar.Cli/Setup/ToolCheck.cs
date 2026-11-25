// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Cli.Shader;

namespace FiveStar.Cli.Setup;

/// <summary>Whether the external shader tools exist where <see cref="ShaderTools"/> looks for them.</summary>
internal static class ToolCheck
{
    private const string PathVariable = "PATH";

    /// <summary>
    /// What is missing of dxc, and of the LLVM tools when <c>--llvm-disassembler</c> asks for them, as one message per
    /// tool with how to provide it.
    /// </summary>
    public static IReadOnlyList<string> MissingShaderTools(CommandArguments arguments)
    {
        var missing = new List<string>();
        var (_, llvm) = ShaderTools.Disassembler(arguments);

        if (!Exists(ShaderTools.Dxc(arguments)))
        {
            missing.Add("dxc not found: install the Windows SDK, or pass --dxc <path to dxc.exe> (or set FIVESTAR_DXC)");
        }

        if (llvm is not null && (!Exists(llvm.Assembler) || !Exists(llvm.Objdump)))
        {
            missing.Add("LLVM (llvm-mc, llvm-objdump) not found: install LLVM 18+, or pass --llvm <bin folder> (or set FIVESTAR_LLVM)");
        }

        return missing;
    }

    /// <summary>Whether the path names a file, or the bare name is found in a PATH folder.</summary>
    private static bool Exists(string tool)
    {
        if (Path.IsPathRooted(tool) || tool.Contains(Path.DirectorySeparatorChar, StringComparison.Ordinal))
        {
            return File.Exists(tool);
        }

        var folders = (Environment.GetEnvironmentVariable(PathVariable) ?? string.Empty).Split(Path.PathSeparator, StringSplitOptions.RemoveEmptyEntries);

        return folders.Any(folder => File.Exists(Path.Combine(folder, tool)));
    }
}
