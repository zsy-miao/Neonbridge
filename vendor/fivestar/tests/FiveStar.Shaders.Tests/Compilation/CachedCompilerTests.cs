// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Compilation;
using Xunit;

namespace FiveStar.Shaders.Tests.Compilation;

/// <summary>The DXIL cache: a source compiles once per cache folder, and a changed source compiles again.</summary>
public sealed class CachedCompilerTests : IDisposable
{
    private static readonly TimeSpan Limit = TimeSpan.FromMinutes(1);

    private readonly DirectoryInfo _folder = Directory.CreateTempSubdirectory("fivestar-cache-");

    public void Dispose()
    {
        _folder.Delete(recursive: true);
    }

    [Fact]
    public void A_second_run_compiles_nothing_and_writes_the_same_dxil()
    {
        var inner = new CountingCompiler();
        string[] sources = ["float4 a;", "float4 b;", "float4 a;", "bad"];
        Run(new CachedCompiler(inner, Cache()), sources, "first");
        var second = Run(new CachedCompiler(inner, Cache()), sources, "second");

        Assert.Equal(3, inner.Compiles);
        Assert.Equal([true, true, true, false], second.Select(outcome => outcome.Succeeded));
        Assert.Equal(File.ReadAllBytes(Hlsl("first", 0) + ".dxil"), File.ReadAllBytes(Hlsl("second", 2) + ".dxil"));
    }

    [Fact]
    public void A_changed_source_compiles_again()
    {
        var inner = new CountingCompiler();
        Run(new CachedCompiler(inner, Cache()), ["float4 a;", "float4 b;"], "first");
        Run(new CachedCompiler(inner, Cache()), ["float4 a;", "float4 c;"], "second");

        Assert.Equal(3, inner.Compiles);
    }

    private List<CompileOutcome> Run(CachedCompiler compiler, string[] sources, string run)
    {
        var outcomes = new List<CompileOutcome>();

        for (var index = 0; index < sources.Length; index++)
        {
            var request = new CompileRequest(Hlsl(run, index) + ".hlsl", sources[index], "ps", DisableOptimizations: false);
            outcomes.Add(compiler.Compile(request, Limit));
        }

        return outcomes;
    }

    private CacheFolder Cache()
    {
        return new CacheFolder(Path.Combine(_folder.FullName, "cache"));
    }

    private string Hlsl(string run, int index)
    {
        return Path.Combine(_folder.FullName, $"{run}{index}");
    }
}
