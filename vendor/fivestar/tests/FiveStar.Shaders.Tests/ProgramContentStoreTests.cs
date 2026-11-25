// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Cache;
using FiveStar.Shaders.Compilation;
using Xunit;

namespace FiveStar.Shaders.Tests;

/// <summary>Compiled programs by what they were compiled from: the same HLSL, profile and compiler find the stored DXIL.</summary>
public sealed class ProgramContentStoreTests : IDisposable
{
    private readonly DirectoryInfo _folder = Directory.CreateTempSubdirectory("fivestar-content-");

    public void Dispose()
    {
        _folder.Delete(recursive: true);
    }

    [Fact]
    public void A_program_compiled_once_is_restored_for_the_same_source_and_compiler_only()
    {
        var store = new ProgramContentStore(Path.Combine(_folder.FullName, ProgramContentStore.FolderName), "backend-a");
        var first = Request("first", "float4 main() : SV_Target { return 1; }");
        var again = Request("again", first.Hlsl);

        Assert.False(store.TryRestore(again));

        File.WriteAllBytes(first.DxilPath, [1, 2, 3]);
        store.Keep(first);

        Assert.True(store.TryRestore(again));
        Assert.Equal([1, 2, 3], File.ReadAllBytes(again.DxilPath));

        // Another source, profile, optimization level or compiler is another key.
        Assert.NotEqual(store.Key(first), store.Key(first with { Hlsl = first.Hlsl + " " }));
        Assert.NotEqual(store.Key(first), store.Key(first with { Profile = "vs" }));
        Assert.NotEqual(store.Key(first), store.Key(first with { DisableOptimizations = true }));
        Assert.NotEqual(store.Key(first), new ProgramContentStore(_folder.FullName, "backend-b").Key(first));
    }

    [Fact]
    public void The_backend_identity_follows_its_sources_not_the_translators_build()
    {
        // Set at build from every source but Translation/: present, and not the module version id that every change alters.
        Assert.Matches("^[0-9A-Fa-f]{16,}$", ProgramContentStore.CompilerSources);
        Assert.NotEqual(typeof(ProgramContentStore).Assembly.ManifestModule.ModuleVersionId.ToString(), ProgramContentStore.CompilerSources);

        var programs = Path.Combine(_folder.FullName, "program-cache", "abcd", "sm6.8-w32");
        Assert.Equal(Path.GetFullPath(Path.Combine(_folder.FullName, "program-cache", ProgramContentStore.FolderName)), ProgramContentStore.FolderBeside(programs));
    }

    private CompileRequest Request(string name, string hlsl)
    {
        return new CompileRequest(Path.Combine(_folder.FullName, name + ".hlsl"), hlsl, "ps", DisableOptimizations: false);
    }
}
