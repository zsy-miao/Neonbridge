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

    /// <param name="inner">The compiler that compiles.</param>
    /// <param name="logPath">The log, shared by every process that compiles into its folder.</param>
    /// <param name="time">The clock.</param>
    /// <param name="processTag">Names this process's own log when another process holds the shared one (its id).</param>
    public TimedCompiler(IShaderCompiler inner, string logPath, TimeProvider time, int processTag)
    {
        _inner = inner;
        _time = time;
        _log = new StreamWriter(Open(logPath, processTag)) { AutoFlush = true };
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

    /// <summary>
    /// The log, appended to; when another process that compiles into the same folder holds it (the warm-up's shards, a
    /// boot beside a warm-up), a log of this process's own beside it (<c>compile-times.&lt;pid&gt;.tsv</c>): opening the one log
    /// threw in the second process and ended it.
    /// </summary>
    private static FileStream Open(string logPath, int processTag)
    {
        try
        {
            return new FileStream(logPath, FileMode.Append, FileAccess.Write, FileShare.Read);
        }
        catch (IOException)
        {
            string own = Path.ChangeExtension(logPath, $"{processTag}{Path.GetExtension(logPath)}");

            return new FileStream(own, FileMode.Append, FileAccess.Write, FileShare.Read);
        }
    }

    /// <summary>The logs in <paramref name="folder"/>: the shared one and those of processes that found it held.</summary>
    public static IEnumerable<string> Logs(string folder)
    {
        if (!Directory.Exists(folder))
        {
            return [];
        }

        return Directory.EnumerateFiles(folder, Path.GetFileNameWithoutExtension(LogName) + "*" + Path.GetExtension(LogName));
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
