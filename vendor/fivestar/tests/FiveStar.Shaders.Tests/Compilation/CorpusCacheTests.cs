// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Compilation;
using FiveStar.Shaders.Disassembly;
using FiveStar.Shaders.Tests.Toolchain;
using Xunit;

namespace FiveStar.Shaders.Tests.Compilation;

/// <summary>
/// Corpus runs on real programs with a cache: a second run reuses every translation and compile and gives the same
/// results.
/// </summary>
public sealed class CorpusCacheTests : IDisposable
{
    private readonly DirectoryInfo _folder = Directory.CreateTempSubdirectory("fivestar-corpus-");

    public void Dispose()
    {
        _folder.Delete(recursive: true);
    }

    [RealProgramTheory]
    [InlineData(4)]
    public void A_second_run_translates_and_compiles_nothing(int count)
    {
        var programs = new DirectoryInfo(RealProgramTheoryAttribute.Folder!)
            .GetFiles("*.fssm")
            .OrderBy(file => file.Length)
            .ThenBy(file => file.Name, StringComparer.Ordinal)
            .Take(count)
            .Select(file => file.FullName)
            .ToList();
        var first = Run(programs, "first");
        var second = Run(programs, "second");

        Assert.Equal(0, second.Translations.Misses);
        Assert.Equal(0, second.Compiler.Compiled);
        Assert.Equal(first.Results, second.Results);
    }

    private (IReadOnlyList<ProgramResult> Results, TranslationCache Translations, CachedCompiler Compiler) Run(List<string> programs, string run)
    {
        var tools = ShaderToolchain.Find();
        var cache = new CacheFolder(Path.Combine(_folder.FullName, "cache"));
        var options = new CorpusOptions(Path.Combine(_folder.FullName, run), Stages: null, Parallelism: 2, RetryResults: null);
        var translations = new TranslationCache(cache, TranslationCache.Identity(null, options.Translation));

        var dxc = DxcOptions.Defaults(tools.Dxc, retry: false);
        var compiler = new CachedCompiler(new DxcCompiler(dxc), cache);
        var loader = new ProgramLoader(new RdnaDecoder());
        var corpus = new CorpusTranslator(loader, compiler, new MeshFallbackCompiler(compiler), translations, progress: null);

        return (corpus.Run(programs, options), translations, compiler);
    }
}
