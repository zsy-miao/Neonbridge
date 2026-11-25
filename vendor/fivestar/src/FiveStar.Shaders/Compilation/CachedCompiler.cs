// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Collections.Concurrent;
using System.Globalization;
using System.Text;
using System.Text.RegularExpressions;

namespace FiveStar.Shaders.Compilation;

/// <summary>
/// A compiler in front of another that keeps outcomes in a <see cref="CacheFolder"/>, keyed by the hash of the inner
/// compiler's identity, the profile, -Od and the HLSL text: the DXIL of a success, the messages of a failure with a
/// source or validation diagnostic, and the time limit of a timeout (reused for limits up to that one). Identical sources compile once, also when they
/// arrive at the same time.
/// </summary>
public sealed partial class CachedCompiler : IShaderCompiler
{
    private const string Kind = "dxil";
    private const string DxilExtension = ".dxil";
    private const string ErrorExtension = ".error";
    private const string TimeoutExtension = ".timeout";
    private const string DisableOptimizationsArgument = "-Od";

    private readonly IShaderCompiler _inner;
    private readonly CacheFolder _folder;
    private readonly ConcurrentDictionary<string, object> _locks = new(StringComparer.Ordinal);
    private int _compiled;
    private int _reused;

    public CachedCompiler(IShaderCompiler inner, CacheFolder folder)
    {
        _inner = inner;
        _folder = folder;
    }

    public DxcOptions Options => _inner.Options;

    public string Identity => _inner.Identity;

    /// <summary>Compiles run by the inner compiler.</summary>
    public int Compiled => Volatile.Read(ref _compiled);

    /// <summary>Compiles answered from the cache.</summary>
    public int Reused => Volatile.Read(ref _reused);

    public CompileOutcome Compile(CompileRequest request, TimeSpan timeout)
    {
        var optimization = request.DisableOptimizations ? DisableOptimizationsArgument : string.Empty;
        var key = ContentHash.OfText(_inner.Identity, request.Profile, optimization, request.Hlsl);

        lock (_locks.GetOrAdd(key, _ => new object()))
        {
            var cached = Find(key, request, timeout);

            if (cached is not null)
            {
                Interlocked.Increment(ref _reused);

                return cached;
            }

            var outcome = _inner.Compile(request, timeout);
            Interlocked.Increment(ref _compiled);
            Store(key, request, outcome, timeout);

            return outcome;
        }
    }

    private CompileOutcome? Find(string key, CompileRequest request, TimeSpan timeout)
    {
        var dxil = _folder.Read(Kind, key, DxilExtension);

        if (dxil is not null)
        {
            File.WriteAllBytes(request.DxilPath, dxil);

            return CompileOutcome.Success;
        }

        var error = _folder.Read(Kind, key, ErrorExtension);

        if (error is not null)
        {
            return CompileOutcome.Failure(Encoding.UTF8.GetString(error));
        }

        var limit = _folder.Read(Kind, key, TimeoutExtension);

        if (limit is not null && TimeSpan.FromSeconds(double.Parse(Encoding.UTF8.GetString(limit), CultureInfo.InvariantCulture)) >= timeout)
        {
            return CompileOutcome.Timeout;
        }

        return null;
    }

    private void Store(string key, CompileRequest request, CompileOutcome outcome, TimeSpan timeout)
    {
        if (outcome.Succeeded)
        {
            _folder.Write(Kind, key, DxilExtension, File.ReadAllBytes(request.DxilPath));
        }
        else if (outcome.TimedOut)
        {
            var seconds = timeout.TotalSeconds.ToString(CultureInfo.InvariantCulture);
            _folder.Write(Kind, key, TimeoutExtension, Encoding.UTF8.GetBytes(seconds));
        }
        else if (SourceDiagnosticPattern().IsMatch(outcome.Error))
        {
            _folder.Write(Kind, key, ErrorExtension, Encoding.UTF8.GetBytes(outcome.Error));
        }
    }

    // A failure without a source or validation diagnostic (dxc crashed or ran out of memory) may not repeat, so it is not kept.
    [GeneratedRegex(@"^(.*:\d+(:\d+)?: error: |error: validation errors)", RegexOptions.Multiline)]
    private static partial Regex SourceDiagnosticPattern();
}
