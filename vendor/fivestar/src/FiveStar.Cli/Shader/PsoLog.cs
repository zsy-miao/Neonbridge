// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Text.Json;

namespace FiveStar.Cli.Shader;

/// <summary>
/// The append-only result log <c>pso-&lt;adapter&gt;.jsonl</c> (the last line of a program wins) and its companion
/// <c>.started</c> file, which lists every pipeline before it is created (and, after <c>-</c>, pipelines whose start a
/// limit took back). A pipeline started in two runs without a result took the process down both times and is recorded
/// as a crash.
/// </summary>
internal sealed class PsoLog : IDisposable
{
    private const int CrashRuns = 2;
    private const string UnstartPrefix = "-";

    private static readonly JsonSerializerOptions Options = new()
    {
        PropertyNamingPolicy = JsonNamingPolicy.CamelCase,
        DefaultIgnoreCondition = System.Text.Json.Serialization.JsonIgnoreCondition.WhenWritingNull,
    };

    private readonly string _resultsPath;
    private readonly string _startedPath;
    private readonly Lock _lock = new();
    private StreamWriter? _results;
    private StreamWriter? _started;

    public PsoLog(string resultsPath)
    {
        _resultsPath = resultsPath;
        _startedPath = Path.ChangeExtension(resultsPath, ".started");
    }

    public string ResultsPath => _resultsPath;

    /// <summary>The latest record of every pipeline, after recording pipelines that crashed twice.</summary>
    public Dictionary<string, PsoRecord> Load()
    {
        var records = ReadRecords();
        var crashed = StartedCounts().Where(pair => pair.Value >= CrashRuns && !records.ContainsKey(pair.Key)).Select(pair => pair.Key).ToList();

        foreach (var key in crashed)
        {
            var parts = key.Split('|');
            var record = new PsoRecord(parts[0], parts[1], PsoRecord.Crashed, null, "process ended during creation", null, 0);
            Append(record);
            records[key] = record;
        }

        return records;
    }

    public void MarkStarted(PsoTarget target)
    {
        lock (_lock)
        {
            _started ??= new StreamWriter(_startedPath, append: true);
            _started.WriteLine(target.Key);
            _started.Flush();
        }
    }

    /// <summary>Takes back a start: the run ends for another pipeline while this one is still being created.</summary>
    public void Unstart(PsoTarget target)
    {
        lock (_lock)
        {
            _started ??= new StreamWriter(_startedPath, append: true);
            _started.WriteLine(UnstartPrefix + target.Key);
            _started.Flush();
        }
    }

    public void Append(PsoRecord record)
    {
        var line = JsonSerializer.Serialize(record, Options);

        lock (_lock)
        {
            _results ??= new StreamWriter(_resultsPath, append: true);
            _results.WriteLine(line);
            _results.Flush();
        }
    }

    public void Dispose()
    {
        _results?.Dispose();
        _started?.Dispose();
    }

    private Dictionary<string, PsoRecord> ReadRecords()
    {
        var records = new Dictionary<string, PsoRecord>(StringComparer.Ordinal);

        if (!File.Exists(_resultsPath))
        {
            return records;
        }

        foreach (var line in Lines(_resultsPath))
        {
            var record = JsonSerializer.Deserialize<PsoRecord>(line, Options);

            if (record is not null)
            {
                records[record.Key] = record;
            }
        }

        return records;
    }

    private Dictionary<string, int> StartedCounts()
    {
        var counts = new Dictionary<string, int>(StringComparer.Ordinal);

        if (!File.Exists(_startedPath))
        {
            return counts;
        }

        foreach (var line in Lines(_startedPath))
        {
            if (line.StartsWith(UnstartPrefix, StringComparison.Ordinal))
            {
                var key = line[UnstartPrefix.Length..];
                counts[key] = counts.GetValueOrDefault(key) - 1;
            }
            else
            {
                counts[line] = counts.GetValueOrDefault(line) + 1;
            }
        }

        return counts;
    }

    /// <summary>The non-empty lines of a file this log may still be writing.</summary>
    private static List<string> Lines(string path)
    {
        using var stream = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.ReadWrite);
        using var reader = new StreamReader(stream);
        var lines = new List<string>();

        for (var line = reader.ReadLine(); line is not null; line = reader.ReadLine())
        {
            if (line.Length > 0)
            {
                lines.Add(line);
            }
        }

        return lines;
    }
}
