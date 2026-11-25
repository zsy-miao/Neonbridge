// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;

namespace FiveStar.Shaders.Compilation;

/// <summary>
/// A compiler in front of another that appends one line per compile it runs to a tab-separated log: source file,
/// profile, -Od, source characters, DXIL bytes, milliseconds and outcome (<c>ok</c>, <c>fail</c>, <c>timeout</c>).
/// Put behind the cache, it times only compiles that ran.
/// </summary>
public sealed class TimedCompiler : IShaderCompiler, IDisposable
{
    /// <summary>The log's file name in the output folder.</summary>
    public const string LogName = "compile-times.tsv";

    private readonly IShaderCompiler _inner;
    private readonly StreamWriter _log;
    private readonly Lock _gate = new();
    private readonly TimeProvider _time;

    public TimedCompiler(IShaderCompiler inner, string logPath, TimeProvider time)
    {
        _inner = inner;
        _time = time;
        _log = new StreamWriter(logPath, append: true) { AutoFlush = true };
    }

    public DxcOptions Options => _inner.Options;

    public string Identity => _inner.Identity;

    public CompileOutcome Compile(CompileRequest request, TimeSpan timeout)
    {
        var started = _time.GetTimestamp();
        var outcome = _inner.Compile(request, timeout);
        var milliseconds = (long)_time.GetElapsedTime(started).TotalMilliseconds;
        var dxil = 0L;

        if (outcome.Succeeded && File.Exists(request.DxilPath))
        {
            dxil = new FileInfo(request.DxilPath).Length;
        }

        var optimization = request.DisableOptimizations ? "-Od" : "-O3";
        var line = string.Create(
            CultureInfo.InvariantCulture,
            $"{Path.GetFileName(request.HlslPath)}\t{request.Profile}\t{optimization}\t{request.Hlsl.Length}\t{dxil}\t{milliseconds}\t{Result(outcome)}");

        lock (_gate)
        {
            _log.WriteLine(line);
        }

        return outcome;
    }

    public void Dispose()
    {
        _log.Dispose();
    }

    private static string Result(CompileOutcome outcome)
    {
        if (outcome.Succeeded)
        {
            return "ok";
        }

        if (outcome.TimedOut)
        {
            return "timeout";
        }

        return "fail";
    }
}
