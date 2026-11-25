// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Dxil;

namespace FiveStar.Shaders.Compilation;

/// <summary>
/// Compiles translated HLSL with the DXIL backend (no dxc; docs/dxil-backend.md), run by an <see cref="IBackendRunner"/>
/// in this process or another. A program the backend does not handle (another stage, or HLSL it cannot lower), or
/// whose container the validator rejects, goes to <c>fallback</c> when there is one, else fails with the backend's
/// reason. Compiles may run on any threads.
/// </summary>
public sealed class DxilBackendCompiler : IShaderCompiler
{
    private const string Prefix = "dxil backend: ";
    private const string ValidatorFile = "dxil.dll";

    private readonly IShaderCompiler? _fallback;
    private readonly IBackendRunner _backend;
    private readonly Lazy<string> _identity;
    private int _compiled;
    private int _unlowered;
    private int _invalid;

    /// <param name="options">Limits, passed on to the fallback.</param>
    /// <param name="fallback">Compiles what the backend cannot; null fails instead.</param>
    /// <param name="backend">Runs the backend.</param>
    public DxilBackendCompiler(DxcOptions options, IShaderCompiler? fallback, IBackendRunner backend)
    {
        Options = options;
        _fallback = fallback;
        _backend = backend;
        _identity = new Lazy<string>(ComputeIdentity);
    }

    public DxcOptions Options { get; }

    public string Identity => _identity.Value;

    /// <summary>Programs the backend compiled, those it could not lower and those the validator rejected (both went to the fallback).</summary>
    public (int Compiled, int Unlowered, int Invalid) Counts => (Volatile.Read(ref _compiled), Volatile.Read(ref _unlowered), Volatile.Read(ref _invalid));

    /// <summary>The counts as a summary: "DXIL backend N, dxc M (U not lowered, I rejected by validation)".</summary>
    public string Summary
    {
        get
        {
            var (compiled, unlowered, invalid) = Counts;

            return $"DXIL backend {compiled}, dxc {unlowered + invalid} ({unlowered} not lowered, {invalid} rejected by validation)";
        }
    }

    /// <summary>The dxil.dll beside a dxc.exe, a dxcompiler.dll or in their folder; null when there is none.</summary>
    public static string? ValidatorBeside(string dxc)
    {
        var path = Path.Combine(Path.GetDirectoryName(DxcLibraryCompiler.LibraryPath(dxc)) ?? ".", ValidatorFile);

        if (!File.Exists(path))
        {
            return null;
        }

        return path;
    }

    public CompileOutcome Compile(CompileRequest request, TimeSpan timeout)
    {
        var result = _backend.Run(request);
        Count(result.Verdict);

        if (result.Verdict == BackendVerdict.Compiled)
        {
            return CompileOutcome.Success;
        }

        return Fallback(request, timeout, result.Reason);
    }

    /// <summary>Counts a verdict the backend reached elsewhere (a program made in the compile host).</summary>
    public void Count(BackendVerdict verdict)
    {
        if (verdict == BackendVerdict.Compiled)
        {
            Interlocked.Increment(ref _compiled);
        }
        else if (verdict == BackendVerdict.Rejected)
        {
            Interlocked.Increment(ref _invalid);
        }
        else
        {
            Interlocked.Increment(ref _unlowered);
        }
    }

    /// <summary>Compiles with the fallback a request the backend did not take (<paramref name="reason"/> when there is none).</summary>
    public CompileOutcome Fallback(CompileRequest request, TimeSpan timeout, string reason)
    {
        if (_fallback is null)
        {
            return CompileOutcome.Failure(Prefix + reason);
        }

        return _fallback.Compile(request, timeout);
    }

    /// <summary>
    /// What the output depends on besides the request: the backend's sources (<see cref="Cache.ProgramContentStore.CompilerSources"/>,
    /// not this assembly's build, which a translator change alters) and whether it validates. The fallback is left out,
    /// so the compile host (which has none) and the loader share stored programs; a source the backend refuses it
    /// refuses again, so dxc's output stands for it under the same key.
    /// </summary>
    public string OutputIdentity => ContentHash.OfText(["dxil-backend", Cache.ProgramContentStore.CompilerSources, ValidationMark]);

    private string ValidationMark
    {
        get
        {
            if (_backend.Validates)
            {
                return "validated";
            }

            return string.Empty;
        }
    }

    /// <summary>This build of the backend (its module version id, a hash of the deterministic build), whether it validates, and the fallback's identity.</summary>
    private string ComputeIdentity()
    {
        var assembly = typeof(DxilBackend).Assembly.ManifestModule.ModuleVersionId.ToString();
        var validated = _backend.Validates ? "validated" : string.Empty;

        return ContentHash.OfText(["dxil-backend", assembly, validated, _fallback?.Identity ?? string.Empty]);
    }
}
