// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Text.Json.Serialization;

namespace FiveStar.Cli.Shader;

/// <summary>
/// One line of <c>pso-&lt;adapter&gt;.jsonl</c>: the program, its kind, the result (<see cref="Accepted"/>,
/// <see cref="Rejected"/>, <see cref="SetupFailed"/>, <see cref="TimedOut"/>, <see cref="OverMemory"/> or <see cref="Crashed"/>), the HRESULT
/// and message of a failure, the pinned wave size of a compute program and the creation time.
/// </summary>
internal sealed record PsoRecord(string Name, string Kind, string Result, string? Hresult, string? Message, int? Wave, long Milliseconds)
{
    /// <summary>The driver created the pipeline state.</summary>
    public const string Accepted = "accepted";

    /// <summary>The runtime or the driver refused the pipeline state.</summary>
    public const string Rejected = "rejected";

    /// <summary>The pipeline could not be described (missing DXIL, pass-through shader failed to compile).</summary>
    public const string SetupFailed = "setup-fail";

    /// <summary>The driver had not finished after the time limit; the run ended there.</summary>
    public const string TimedOut = "timeout";

    /// <summary>The process passed the memory limit while only this pipeline was being created; the run ended there.</summary>
    public const string OverMemory = "over-memory";

    /// <summary>The process ended while creating this pipeline state in two runs.</summary>
    public const string Crashed = "crash";

    [JsonIgnore]
    public string Key => $"{Name}|{Kind}";
}
