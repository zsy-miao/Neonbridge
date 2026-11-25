// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Compilation;
using FiveStar.Shaders.Disassembly;
using FiveStar.Shaders.Translation;

namespace FiveStar.Cli.Shader;

/// <summary>
/// The identity a shader-cache usage record is kept under: the translator identity (FiveStar.Shaders, the LLVM tools
/// when they disassemble, the translation options) and the compiler identity (dxc and its fixed arguments), or
/// "no-compile".
/// </summary>
internal static class ShaderCacheIdentity
{
    private const string NoCompiler = "no-compile";

    public static string Of(LlvmToolPaths? llvm, TranslationOptions translation, IShaderCompiler? compiler)
    {
        var compilerIdentity = NoCompiler;

        if (compiler is not null)
        {
            compilerIdentity = compiler.Identity;
        }

        return ContentHash.OfText(TranslationCache.Identity(llvm, translation), compilerIdentity);
    }
}
