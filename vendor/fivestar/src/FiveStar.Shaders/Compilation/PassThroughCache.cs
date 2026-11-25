// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Collections.Concurrent;
using System.Security.Cryptography;
using System.Text;

namespace FiveStar.Shaders.Compilation;

/// <summary>
/// Compiles generated pass-through vertex shaders once per source text: <c>&lt;folder&gt;/vs_&lt;hash&gt;.hlsl</c> and
/// <c>.dxil</c>, reused from disk by later runs. Safe to call from several threads.
/// </summary>
public sealed class PassThroughCache
{
    private const int HashCharacters = 16;

    private readonly DxcCompiler _compiler;
    private readonly string _folder;
    private readonly ConcurrentDictionary<string, Lazy<byte[]>> _shaders = new(StringComparer.Ordinal);

    public PassThroughCache(DxcCompiler compiler, string folder)
    {
        _compiler = compiler;
        _folder = folder;
    }

    /// <summary>The DXIL of the source; throws <see cref="InvalidOperationException"/> with dxc's first error when it does not compile.</summary>
    public byte[] Compile(string source)
    {
        var key = Convert.ToHexStringLower(SHA256.HashData(Encoding.UTF8.GetBytes(source)))[..HashCharacters];

        return _shaders.GetOrAdd(key, _ => new Lazy<byte[]>(() => CompileFile(key, source))).Value;
    }

    private byte[] CompileFile(string key, string source)
    {
        Directory.CreateDirectory(_folder);
        var hlslPath = Path.Combine(_folder, $"vs_{key}.hlsl");
        var dxilPath = Path.ChangeExtension(hlslPath, ".dxil");

        if (File.Exists(dxilPath))
        {
            return File.ReadAllBytes(dxilPath);
        }

        File.WriteAllText(hlslPath, source);
        var request = new CompileRequest(hlslPath, source, "vs", DisableOptimizations: false);
        var outcome = _compiler.Compile(request, _compiler.Options.CompileTimeout);

        if (!outcome.Succeeded)
        {
            throw new InvalidOperationException("pass-through VS: " + DxcCompiler.FirstError(outcome.Error));
        }

        return File.ReadAllBytes(request.DxilPath);
    }
}
