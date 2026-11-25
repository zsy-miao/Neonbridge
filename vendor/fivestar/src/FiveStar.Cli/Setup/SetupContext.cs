// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Cli.Shader;

namespace FiveStar.Cli.Setup;

/// <summary>
/// What every step works from: the dump, the work folder, the sample size (<c>--limit</c> programs and textures per
/// file, every <c>--step</c>-th program), the parallel jobs, and the command's arguments for the tool options.
/// </summary>
internal sealed record SetupContext(string Dump, string Work, int? Limit, int Every, int Jobs, CommandArguments Arguments)
{
    public string ShaderFolder => Path.Combine(Work, "shaders");

    public string ProgramFolder => Path.Combine(ShaderFolder, "fssm");

    public string ManifestPath => Path.Combine(ShaderFolder, "dxbc", "_manifest.tsv");

    public string TranslationFolder => Path.Combine(ShaderFolder, "hlsl");

    public string CacheFolder => Path.Combine(Work, "shader-cache");

    public string PipelineFolder => Path.Combine(Work, "pipelines");

    /// <summary>The loader's disk cache of built desktop pipelines (<c>fivestar-loader --pipeline-cache</c>).</summary>
    public string PipelineLibraryFolder => Path.Combine(Work, "pipeline-cache");

    /// <summary>The loader's programs translated and compiled on first use (<c>fivestar-loader --program-cache</c>).</summary>
    public string ProgramCacheFolder => Path.Combine(Work, "program-cache");

    public string TextureFolder => Path.Combine(Work, "textures");

    public string LoaderFolder => Path.Combine(Work, "loader");

    /// <summary>The thunk ABI table of the <c>gdk-abi</c> step, for <c>fivestar-loader --gdk-abi</c>.</summary>
    public string AbiTable => Path.Combine(LoaderFolder, "gdk-abi.txt");

    /// <summary>The user's BCPack tables as the <c>bcpack-tables</c> step imported them, for <c>fivestar-loader --bcpack-tables</c>.</summary>
    public string BcPackTables => Path.Combine(LoaderFolder, Formats.Ttpf.BcPack.BcPackTables.FileName);

    /// <summary>Where <c>fivestar run</c> lets the loader write its TLS host DLLs (<c>--work-dir</c>).</summary>
    public string TlsFolder => Path.Combine(LoaderFolder, "tls");

    /// <summary>The loader logs of <c>fivestar run</c>, one per run.</summary>
    public string LogFolder => Path.Combine(Work, "logs");

    public string StateFolder => Path.Combine(Work, "state");

    public string ShaderArchive => Path.Combine(Dump, "shaders.rpf");

    /// <summary>The game executable, which embeds a few shader programs of its own.</summary>
    public string Executable => Path.Combine(Dump, Run.RunPlan.GameExecutable);

    /// <summary>The sample options as text, for fingerprints: a different sample redoes the work.</summary>
    public string Sample => $"limit={Limit} step={Every}";
}
