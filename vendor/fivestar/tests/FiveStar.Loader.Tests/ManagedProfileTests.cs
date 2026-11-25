// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Text;
using FiveStar.Loader.Diagnostics;
using Xunit;

namespace FiveStar.Loader.Tests;

/// <summary>--profile-managed: the process traces itself through the diagnostics IPC, and threads get their class.</summary>
public sealed class ManagedProfileTests
{
    private const int SampleMilliseconds = 200;

    /// <summary>A .nettrace starts with its magic and the serializer's name.</summary>
    private const string NetTraceStart = "Nettrace";

    [Fact]
    public void A_session_of_the_process_itself_writes_a_nettrace_with_its_rundown()
    {
        string path = Path.Combine(Path.GetTempPath(), $"fivestar-session-{Guid.NewGuid():N}.nettrace");

        try
        {
            long bytes;

            using (var session = EventPipeSession.Start(path, [(EventPipeSession.SampleProfiler, 0, EventPipeSession.Verbose), (EventPipeSession.Runtime, EventPipeSession.MethodKeywords, EventPipeSession.Verbose)]))
            {
                Thread.Sleep(SampleMilliseconds);
                session.Stop();
                bytes = session.Bytes;
            }

            var start = new byte[NetTraceStart.Length];

            using (var file = File.OpenRead(path))
            {
                file.ReadExactly(start);
            }

            Assert.Equal(NetTraceStart, Encoding.ASCII.GetString(start));
            Assert.Equal(new FileInfo(path).Length, bytes);
            Assert.Contains("Microsoft-Windows-DotNETRuntimeRundown", Encoding.Unicode.GetString(File.ReadAllBytes(path)), StringComparison.Ordinal);
        }
        finally
        {
            File.Delete(path);
        }
    }

    [Fact]
    public void The_option_takes_a_start_and_a_length_and_threads_get_a_class()
    {
        Assert.True(ManagedProfile.TryParse("120:30", out var start, out var length));
        Assert.Equal((TimeSpan.FromSeconds(120), TimeSpan.FromSeconds(30)), (start, length));
        Assert.True(ManagedProfile.TryParse("5", out _, out length));
        Assert.Equal(TimeSpan.FromSeconds(20), length);
        Assert.False(ManagedProfile.TryParse("5:0", out _, out _));

        Assert.Equal("game", ManagedProfile.Classify(startsInGame: true, string.Empty));
        Assert.Equal("build", ManagedProfile.Classify(startsInGame: false, "fivestar program compile"));
        Assert.Equal("queue", ManagedProfile.Classify(startsInGame: false, "fivestar queue progress"));
        Assert.Equal("runtime", ManagedProfile.Classify(startsInGame: false, ".NET TP Worker"));
        Assert.Equal("loader", ManagedProfile.Classify(startsInGame: false, "fivestar hang watch"));
        Assert.Equal("other", ManagedProfile.Classify(startsInGame: false, string.Empty));
    }
}
