// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using Microsoft.Diagnostics.Tracing.Etlx;
using Microsoft.Diagnostics.Tracing.EventPipe;

namespace FiveStar.Cli.Profile;

/// <summary>
/// The CPU samples of a loader managed profile (<c>--profile-managed</c>, a .nettrace of the runtime's sample
/// profiler) by thread class: per class the samples in managed code, the samples outside it (native code or waiting),
/// and the methods by exclusive (innermost frame, past the runtime's GC poll a sampled loop is often stopped in) and
/// inclusive (anywhere on the stack) samples. The classes come from the .threads.tsv the loader writes beside the
/// trace (thread id, class, name); threads it does not list are <see cref="Unlisted"/>.
/// </summary>
internal sealed class ManagedProfileSummary
{
    public const string ThreadsExtension = ".threads.tsv";
    public const string Unlisted = "unlisted";
    public const string All = "all";

    private const double Percent = 100.0;
    private const string GcPoll = "PollGC";
    private const int CountWidth = 7;
    private const int ShareWidth = 5;
    private const int IdAndClass = 2;

    private ManagedProfileSummary(IReadOnlyList<ProfileClass> classes)
    {
        Classes = classes;
    }

    /// <summary>The classes, busiest first, then <see cref="All"/>.</summary>
    public IReadOnlyList<ProfileClass> Classes { get; }

    /// <summary>Reads a trace and the thread table beside it (none: every thread <see cref="Unlisted"/>).</summary>
    public static ManagedProfileSummary Read(string trace)
    {
        var threads = ReadThreads(Path.ChangeExtension(trace, null) + ThreadsExtension);
        var etlx = Path.Combine(Path.GetTempPath(), $"fivestar-profile-{Guid.NewGuid():N}.etlx");
        var classes = new Dictionary<string, ProfileClass>(StringComparer.Ordinal);
        var all = new ProfileClass(All);

        try
        {
            TraceLog.CreateFromEventPipeDataFile(trace, etlx);

            using var log = new TraceLog(etlx);

            foreach (var sample in log.Events.ByEventType<ClrThreadSampleTraceData>())
            {
                var kind = threads.GetValueOrDefault(sample.ThreadID, Unlisted);

                if (!classes.TryGetValue(kind, out var profileClass))
                {
                    profileClass = new ProfileClass(kind);
                    classes[kind] = profileClass;
                }

                var frames = Frames(sample.CallStack());
                bool managed = sample.Type == ClrThreadSampleType.Managed;
                profileClass.Add(sample.ThreadID, managed, frames);
                all.Add(sample.ThreadID, managed, frames);
            }
        }
        finally
        {
            File.Delete(etlx);
        }

        return new ManagedProfileSummary([.. classes.Values.OrderByDescending(profileClass => profileClass.Managed), all]);
    }

    /// <summary>Per class its sample counts and its <paramref name="top"/> methods, exclusive then inclusive.</summary>
    public void Write(TextWriter output, int top)
    {
        foreach (var profileClass in Classes)
        {
            output.WriteLine($"== {profileClass.Name}: {profileClass.Managed} samples in managed code, {profileClass.External} outside it, {profileClass.Threads.Count} threads");
            WriteTop(output, "exclusive", profileClass.Exclusive, profileClass.Managed, top);
            WriteTop(output, "inclusive", profileClass.Inclusive, profileClass.Managed, top);
        }
    }

    private static void WriteTop(TextWriter output, string title, Dictionary<string, int> counts, int samples, int top)
    {
        output.WriteLine($"  {title}:");

        foreach (var (method, count) in counts.OrderByDescending(pair => pair.Value).ThenBy(pair => pair.Key, StringComparer.Ordinal).Take(top))
        {
            output.WriteLine(string.Create(CultureInfo.InvariantCulture, $"    {count,CountWidth} {Percent * count / Math.Max(1, samples),ShareWidth:F1} %  {method}"));
        }
    }

    /// <summary>The stack's methods, innermost first, as <c>module!Type.Method</c> without the signature.</summary>
    private static List<string> Frames(TraceCallStack? stack)
    {
        var frames = new List<string>();

        for (; stack is not null; stack = stack.Caller)
        {
            var address = stack.CodeAddress;
            string method = address.FullMethodName;
            int signature = method.IndexOf('(', StringComparison.Ordinal);

            if (signature > 0)
            {
                method = method[..signature];
            }

            if (method.Length == 0)
            {
                method = "?";
            }

            // A thread the sampler stops in a loop is often stopped in the runtime's GC poll: the loop is the sample.
            if (frames.Count == 0 && method.Contains(GcPoll, StringComparison.Ordinal))
            {
                continue;
            }

            frames.Add($"{Module(address.ModuleName)}!{method}");
        }

        return frames;
    }

    /// <summary>The module's name without the <c>.il</c> TraceEvent gives ReadyToRun images.</summary>
    private static string Module(string module)
    {
        const string IlSuffix = ".il";

        if (module.EndsWith(IlSuffix, StringComparison.OrdinalIgnoreCase))
        {
            return module[..^IlSuffix.Length];
        }

        return module;
    }

    private static Dictionary<int, string> ReadThreads(string path)
    {
        var threads = new Dictionary<int, string>();

        if (!File.Exists(path))
        {
            return threads;
        }

        foreach (var line in File.ReadLines(path))
        {
            var fields = line.Split('\t');

            if (fields.Length >= IdAndClass && int.TryParse(fields[0], NumberStyles.Integer, CultureInfo.InvariantCulture, out int id))
            {
                threads[id] = fields[1];
            }
        }

        return threads;
    }
}
