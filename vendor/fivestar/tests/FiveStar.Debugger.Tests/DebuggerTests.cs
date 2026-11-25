// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Text.RegularExpressions;
using FiveStar.Debugger.Session;
using Xunit;

namespace FiveStar.Debugger.Tests;

/// <summary>Runs <see cref="CrashTarget"/> under the debugger in this process (Windows x64).</summary>
public sealed class DebuggerTests
{
    private const int AccessViolationExit = unchecked((int)0xC0000005);

    private static readonly string Target = Path.Combine(AppContext.BaseDirectory, "FiveStar.Debugger.Tests.exe");

    [Fact]
    public void AccessViolationIsReportedWithModuleSymbolisedStackAndDump()
    {
        string dump = Path.Combine(Path.GetTempPath(), $"fivestar-dbg-test-{Guid.NewGuid():N}.dmp");
        var options = Options(CrashTarget.AccessViolation, false) with { BreakOn = new HashSet<uint> { 0xC0000005 }, DumpFile = dump };

        try
        {
            var (exitCode, output) = Run(options);

            Assert.Equal(AccessViolationExit, exitCode);
            Assert.Matches(@"first-chance c0000005 AccessViolation at 0x[0-9a-f]+ ntdll\.dll\+0x[0-9a-f]+ ntdll!\S+ .*\(write at 0x10\)", output);
            Assert.Matches(new Regex(@"#00 0x[0-9a-f]{16} ntdll\.dll\+0x[0-9a-f]+ ntdll!", RegexOptions.Multiline), output);
            Assert.Equal("MDMP"u8.ToArray(), File.ReadAllBytes(dump)[..4]);
        }
        finally
        {
            File.Delete(dump);
        }
    }

    [Fact]
    public void DebugHeapFaultsOnTheOverrunningWriteThatTheNormalHeapLetsThrough()
    {
        var (normalExit, normalOutput) = Run(Options(CrashTarget.HeapOverrun, false));
        var (debugExit, debugOutput) = Run(Options(CrashTarget.HeapOverrun, true));

        Assert.Equal(0, normalExit);
        Assert.DoesNotContain("AccessViolation", normalOutput, StringComparison.Ordinal);
        Assert.Equal(AccessViolationExit, debugExit);
        Assert.Contains("page heap on", debugOutput, StringComparison.Ordinal);
        Assert.Matches(@"first-chance c0000005 AccessViolation at 0x[0-9a-f]+ ntdll\.dll\+0x[0-9a-f]+ .*\(write at 0x[0-9a-f]+000\)", debugOutput);
    }

    /// <summary>No symbol path, so a developer's _NT_SYMBOL_PATH does not make the tests download.</summary>
    private static DebuggerOptions Options(string mode, bool debugHeap)
    {
        return new DebuggerOptions(Target, [mode], debugHeap, false, new HashSet<uint>(), string.Empty, null, false, null, null, TimeSpan.FromMinutes(1));
    }

    private static (int ExitCode, string Output) Run(DebuggerOptions options)
    {
        using var output = new StringWriter();
        using var log = new DebugLog(output, null);
        int exitCode = Program.Run(options, log);

        return (exitCode, output.ToString());
    }
}
