// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Reflection;
using FiveStar.Shaders.Compilation;
using FiveStar.Shaders.Translation;

namespace FiveStar.Shaders.Cache;

/// <summary>
/// Compiled programs by what they were compiled from: <c>&lt;folder&gt;/&lt;aa&gt;/&lt;key&gt;.dxil</c>, the key a
/// hash of the HLSL, the profile, the optimization level, the runtime headers it includes, the compiler (dxc's files, or
/// the DXIL backend's sources: every source of this assembly but the translator's, <see cref="CompilerSources"/>) and
/// whether the output is validated. The loader's program folders are named by the translator's build, so each translator
/// change starts an empty one; with the store a program whose HLSL that change left alone is copied from here instead of
/// compiled again, and its pipelines, keyed by DXIL, stay in the pipeline cache.
/// </summary>
public sealed class ProgramContentStore
{
    /// <summary>The store's folder name under the program cache.</summary>
    public const string FolderName = "content";

    private const string Extension = ".dxil";
    private const string TemporarySuffix = ".tmp";
    private const int ShardLength = 2;
    private const string MetadataKey = "CompilerSources";

    private readonly string _folder;
    private readonly string _identity;

    /// <param name="folder">The store's folder.</param>
    /// <param name="compilerIdentity">What the compiler's output depends on besides the request (<see cref="CompilerIdentity"/>).</param>
    public ProgramContentStore(string folder, string compilerIdentity)
    {
        _folder = folder;
        _identity = ContentHash.OfText([compilerIdentity, .. RuntimeIncludes.Names.Select(RuntimeIncludes.Read)]);
    }

    /// <summary>The hash of the sources of the DXIL backend and the rest of this assembly but the translator (set at build).</summary>
    public static string CompilerSources { get; } =
        typeof(ProgramContentStore).Assembly.GetCustomAttributes<AssemblyMetadataAttribute>().FirstOrDefault(attribute => attribute.Key == MetadataKey)?.Value
        ?? typeof(ProgramContentStore).Assembly.ManifestModule.ModuleVersionId.ToString();

    /// <summary>
    /// What a compiler's output depends on besides the request: the DXIL backend's sources and validation (not this
    /// assembly's build, which every translator change alters), or another compiler's own identity (dxc's files).
    /// </summary>
    public static string CompilerIdentity(IShaderCompiler compiler)
    {
        if (compiler is DxilBackendCompiler backend)
        {
            return backend.OutputIdentity;
        }

        return compiler.Identity;
    }

    /// <summary>The store beside a program folder <c>&lt;program cache&gt;/&lt;identity&gt;/&lt;adapter&gt;</c>.</summary>
    public static string FolderBeside(string programFolder)
    {
        return Path.GetFullPath(Path.Combine(programFolder, "..", "..", FolderName));
    }

    /// <summary>The key of <paramref name="request"/>.</summary>
    public string Key(CompileRequest request)
    {
        string level = request.DisableOptimizations ? "-Od" : "-O3";

        return ContentHash.OfText([_identity, request.Profile, level, request.Hlsl]);
    }

    /// <summary>Writes the stored DXIL of <paramref name="request"/> to its <see cref="CompileRequest.DxilPath"/>; false when none is stored.</summary>
    public bool TryRestore(CompileRequest request)
    {
        var path = PathOf(Key(request));

        if (!File.Exists(path))
        {
            return false;
        }

        try
        {
            File.Copy(path, request.DxilPath, overwrite: true);

            return true;
        }
        catch (Exception exception) when (exception is IOException or UnauthorizedAccessException)
        {
            return false;
        }
    }

    /// <summary>Keeps the DXIL a compile of <paramref name="request"/> wrote; a store that cannot be written keeps nothing.</summary>
    public void Keep(CompileRequest request)
    {
        var path = PathOf(Key(request));

        if (File.Exists(path) || !File.Exists(request.DxilPath))
        {
            return;
        }

        var temporary = path + "." + Guid.NewGuid().ToString("N") + TemporarySuffix;

        try
        {
            Directory.CreateDirectory(Path.GetDirectoryName(path)!);
            File.Copy(request.DxilPath, temporary);
            File.Move(temporary, path, overwrite: true);
        }
        catch (Exception exception) when (exception is IOException or UnauthorizedAccessException)
        {
            File.Delete(temporary);
        }
    }

    private string PathOf(string key)
    {
        return Path.Combine(_folder, key[..ShardLength], key + Extension);
    }
}
