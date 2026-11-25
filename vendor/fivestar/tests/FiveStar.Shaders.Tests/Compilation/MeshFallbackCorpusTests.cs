// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Compilation;
using FiveStar.Shaders.Disassembly;
using FiveStar.Shaders.Tests.Differential;
using FiveStar.Shaders.Tests.Toolchain;
using FiveStar.Shaders.Translation;
using Xunit;

namespace FiveStar.Shaders.Tests.Compilation;

/// <summary>With <see cref="CorpusOptions.MeshFallbacks"/> a mesh program that compiles natively gets its fallback too.</summary>
public sealed class MeshFallbackCorpusTests : IDisposable
{
    private readonly DirectoryInfo _folder = Directory.CreateTempSubdirectory("fivestar-msfb-");

    public void Dispose()
    {
        _folder.Delete(recursive: true);
    }

    [RealProgramTheory]
    [InlineData(2)]
    public void Every_mesh_program_gets_its_fallback_when_asked(int count)
    {
        var programs = ProgramSample.Of(RealProgramTheoryAttribute.Folder!, ShaderStage.Mesh, count);
        var compiler = new CountingCompiler();
        var options = new CorpusOptions(_folder.FullName, Stages: null, Parallelism: 1, RetryResults: null) { MeshFallbacks = true };
        var corpus = new CorpusTranslator(new ProgramLoader(new RdnaDecoder()), compiler, new MeshFallbackCompiler(compiler), translations: null, progress: null);

        var results = corpus.Run(programs, options);

        Assert.All(results, result => Assert.Equal((ProgramStatus.Ok, CorpusTranslator.MeshFallbackReady), (result.Status, result.Reason)));
        Assert.All(results, result => Assert.True(File.Exists(Path.Combine(_folder.FullName, result.Name + ".msfb_cs.dxil"))));
        Assert.Equal(3 * count, compiler.Compiles);
    }
}
