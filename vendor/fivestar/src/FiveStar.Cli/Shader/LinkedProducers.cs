// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Collections.Concurrent;
using FiveStar.Graphics;
using FiveStar.Graphics.Programs;
using FiveStar.Shaders.Compilation;
using FiveStar.Shaders.Containers;
using FiveStar.Shaders.Translation;

namespace FiveStar.Cli.Shader;

/// <summary>
/// Prepares the pipeline of a pair on one adapter: the producer form the adapter runs (the vertex shader, the mesh
/// shader, or the mesh fallback's vertex shader where it runs the fallback) linked to the pixel shader's interpolation
/// modes (<see cref="StageLinker"/>). A producer the link leaves unchanged keeps its DXIL; a changed one is compiled
/// once into <c>&lt;out&gt;/linked/&lt;name&gt;.&lt;hash&gt;.dxil</c>.
/// </summary>
internal sealed class LinkedProducers
{
    public const string LinkedFolder = "linked";

    private const string Vertex = "vs";
    private const string Mesh = "ms";
    private const string FallbackVertex = ".msfb_vs";
    private const int HashDigits = 12;

    private readonly string _folder;
    private readonly string _linkedFolder;
    private readonly IShaderCompiler _compiler;
    private readonly AdapterInfo _adapter;
    private readonly bool _unlinked;
    private readonly ConcurrentDictionary<string, Lazy<string?>> _compiles = new(StringComparer.Ordinal);
    private int _linked;
    private int _unchanged;

    /// <summary>With <paramref name="unlinked"/> every producer keeps its DXIL, for comparison.</summary>
    public LinkedProducers(string folder, IShaderCompiler compiler, AdapterInfo adapter, bool unlinked)
    {
        _folder = folder;
        _linkedFolder = Path.Combine(folder, LinkedFolder);
        _compiler = compiler;
        _adapter = adapter;
        _unlinked = unlinked;
        RuntimeIncludes.WriteTo(_linkedFolder);
    }

    /// <summary>Gets the number of producers the link changed.</summary>
    public int Linked => _linked;

    /// <summary>Gets the number of producers whose outputs already had the pixel shader's modes.</summary>
    public int Unchanged => _unchanged;

    /// <summary>The pipeline kind a pair has before linking: vs+ps, ms+ps or msfb_vs+ps.</summary>
    public string Kind(LinkPair pair)
    {
        return Form(pair, fallback: false).Kind;
    }

    /// <summary>
    /// The pair's pipeline target; a <see cref="PipelineSetupException"/> when the link or its compile fails. A linked
    /// mesh shader that no longer compiles (the added outputs pass the 32 KB mesh output limit) runs its fallback.
    /// </summary>
    public PsoTarget Prepare(LinkPair pair)
    {
        var form = Form(pair, fallback: false);

        try
        {
            return Prepare(pair, form);
        }
        catch (PipelineSetupException) when (form.Profile == Mesh && pair.Producer.MeshFallback is not null)
        {
            return Prepare(pair, Form(pair, fallback: true));
        }
    }

    private PsoTarget Prepare(LinkPair pair, (string Kind, string Stem, string Profile) form)
    {
        var consumer = Path.Combine(_folder, pair.Consumer.Dxil!);
        var target = new PsoTarget(pair.Name, form.Kind, Path.Combine(_folder, form.Stem + ".dxil")) { ConsumerPath = consumer };

        if (_unlinked)
        {
            return target;
        }

        var hlslPath = Path.Combine(_folder, form.Stem + ".hlsl");

        if (!File.Exists(hlslPath) || !File.Exists(consumer))
        {
            throw new PipelineSetupException("producer HLSL or pixel shader DXIL missing");
        }

        var source = File.ReadAllText(hlslPath);
        var linked = Link(source, File.ReadAllBytes(consumer));

        if (linked == source)
        {
            Interlocked.Increment(ref _unchanged);

            return target;
        }

        var dxil = Compile(Path.GetFileName(form.Stem), linked, form.Profile);
        Interlocked.Increment(ref _linked);

        return target with { DxilPath = dxil };
    }

    /// <summary>The producer form the adapter runs: kind, file stem and profile.</summary>
    private (string Kind, string Stem, string Profile) Form(LinkPair pair, bool fallback)
    {
        var stem = pair.Producer.Name;

        if (pair.Producer.Stage != Mesh)
        {
            return ($"{Vertex}+ps", stem, Vertex);
        }

        if (fallback || VariantSelector.Choose(_adapter, pair.Producer).Files.Count > 1)
        {
            return ($"msfb_{Vertex}+ps", stem + FallbackVertex, Vertex);
        }

        return ($"{Mesh}+ps", stem, Mesh);
    }

    private static string Link(string producer, byte[] consumer)
    {
        try
        {
            return StageLinker.Link(producer, PsvSignatureParser.Parse(ProgramChunks.Psv(consumer).Span).Inputs);
        }
        catch (UnsupportedProgramException exception)
        {
            throw new PipelineSetupException("link: " + exception.Message);
        }
    }

    /// <summary>Compiles a linked producer once (pairs that link it the same way share it); returns its DXIL path.</summary>
    private string Compile(string name, string hlsl, string profile)
    {
        var path = Path.Combine(_linkedFolder, $"{name}.{ContentHash.OfText(hlsl)[..HashDigits]}.hlsl");
        var error = _compiles.GetOrAdd(path, _ => new Lazy<string?>(() => Run(path, hlsl, profile))).Value;

        if (error is not null)
        {
            throw new PipelineSetupException(error);
        }

        return Path.ChangeExtension(path, ".dxil");
    }

    private string? Run(string path, string hlsl, string profile)
    {
        if (File.Exists(Path.ChangeExtension(path, ".dxil")))
        {
            return null;
        }

        File.WriteAllText(path, hlsl);
        var options = _compiler.Options;
        var outcome = _compiler.Compile(new CompileRequest(path, hlsl, profile, hlsl.Length > options.LargeSourceLength), options.CompileTimeout);

        if (outcome.TimedOut)
        {
            outcome = _compiler.Compile(new CompileRequest(path, hlsl, profile, DisableOptimizations: true), options.RecompileTimeout);
        }

        if (outcome.Succeeded)
        {
            return null;
        }

        if (outcome.TimedOut)
        {
            return "linked producer: dxc timeout";
        }

        return "linked producer: " + DxcCompiler.FirstError(outcome.Error);
    }
}
