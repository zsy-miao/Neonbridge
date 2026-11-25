// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Diagnostics;
using System.Diagnostics.Tracing;
using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;
using FiveStar.Cli.Profile;
using Microsoft.Diagnostics.NETCore.Client;
using Xunit;

namespace FiveStar.Cli.Tests;

/// <summary>
/// A small managed profile of this process, recorded the way the loader records one (sample profiler, method events,
/// rundown) while a thread the table calls "build" spins in one method: the summary names that method among the top of
/// the class, exclusive and inclusive. Among the top, not first: on a loaded CI machine the samples of the thread's start
/// (JIT, thread setup) or of a helper the loop calls can outnumber a short burn's, which failed PR #26's Windows run.
/// </summary>
public sealed partial class ManagedProfileSummaryTests
{
    private const int BurnMilliseconds = 1500;
    private const int TopExclusive = 3;
    private const int Top = 5;

    [Fact]
    public void A_profile_names_the_method_a_thread_class_spends_its_samples_in()
    {
        // The thread table keys threads by their Windows thread id, as the loader writes it.
        if (!OperatingSystem.IsWindows())
        {
            return;
        }

        var folder = Directory.CreateTempSubdirectory("fivestar-profile-");

        try
        {
            string trace = Path.Combine(folder.FullName, "boot.managed.nettrace");
            uint burner = Record(trace);
            File.WriteAllText(Path.Combine(folder.FullName, "boot.managed" + ManagedProfileSummary.ThreadsExtension), $"{burner}\tbuild\tfivestar program compile\n");

            var summary = ManagedProfileSummary.Read(trace);
            var build = summary.Classes.Single(profileClass => profileClass.Name == "build");
            var output = new StringWriter();
            summary.Write(output, Top);

            var leading = build.Exclusive.OrderByDescending(pair => pair.Value).Take(TopExclusive).Select(pair => pair.Key);
            Assert.Contains(leading, method => method.EndsWith("." + nameof(BurnCpu), StringComparison.Ordinal));
            Assert.Contains(build.Inclusive.Keys, method => method.EndsWith("." + nameof(BurnCpu), StringComparison.Ordinal));
            Assert.Contains("== build: ", output.ToString(), StringComparison.Ordinal);
            Assert.Contains(nameof(BurnCpu), output.ToString(), StringComparison.Ordinal);
            Assert.Equal(ManagedProfileSummary.All, summary.Classes[^1].Name);
        }
        finally
        {
            folder.Delete(recursive: true);
        }
    }

    /// <summary>Records this process while a thread burns CPU; returns that thread's id.</summary>
    private static uint Record(string trace)
    {
        var providers = new[]
        {
            new EventPipeProvider("Microsoft-DotNETCore-SampleProfiler", EventLevel.Verbose),
            new EventPipeProvider("Microsoft-Windows-DotNETRuntime", EventLevel.Verbose, 0x38),
        };
        uint burner = 0;

        using var session = new DiagnosticsClient(Environment.ProcessId).StartEventPipeSession(providers, requestRundown: true);
        var copy = Task.Run(() =>
        {
            using var file = File.Create(trace);
            session.EventStream.CopyTo(file);
        });
        var thread = new Thread(() =>
        {
            burner = GetCurrentThreadId();
            BurnCpu();
        });

        thread.Start();
        thread.Join();
        session.Stop();
        copy.Wait();

        return burner;
    }

    [MethodImpl(MethodImplOptions.NoInlining)]
    private static void BurnCpu()
    {
        // The clock is read once per many iterations, so the samples land in this method and not in Stopwatch (which
        // a Debug build does not inline).
        const int Iterations = 1 << 16;
        var clock = Stopwatch.StartNew();
        double value = 1;

        while (clock.ElapsedMilliseconds < BurnMilliseconds)
        {
            for (int index = 0; index < Iterations; index++)
            {
                value = Math.Sqrt(value + index);
            }
        }

        GC.KeepAlive(value);
    }

    [LibraryImport("kernel32.dll")]
    private static partial uint GetCurrentThreadId();
}
