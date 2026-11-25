// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Cli.Exe;
using FiveStar.Cli.Tests.Support;
using Xunit;

namespace FiveStar.Cli.Tests;

/// <summary>
/// <c>fivestar exe analyze</c> on the package's own DLLs and executable (FIVESTAR_DUMP). The GDK thunk counts are the
/// ones found by hand in the disassembly: one QueryApiImpl call per thunk.
/// </summary>
public sealed class ExeAnalyzeTests
{
    private const string HttpClient = "libHttpClient.GDK.dll";
    private const string GameExecutable = "game_scarlett_master.exe";
    private const int HttpClientThunks = 404;

    private static readonly string[] PackageDlls =
    [
        "amd_fidelityfx_loader_xbox.dll", "amd_fidelityfx_upscaler_xbox.dll", "Bink2Scarlett.dll",
        HttpClient, "XboxBvhBuilder.dll", "XCurl.dll",
    ];

    [DumpFact]
    public void Gdk_thunks_are_found_without_the_library()
    {
        Assert.Equal(HttpClientThunks, Analyze(HttpClient, null).GdkThunkCount);
        Assert.Equal(16, Analyze("XCurl.dll", null).GdkThunkCount);

        // The executable resolves QueryApiImpl at run time instead of importing it.
        if (File.Exists(Path.Combine(DumpFactAttribute.Folder!, GameExecutable)))
        {
            var game = Analyze(GameExecutable, null);

            Assert.Equal(161, game.GdkThunkCount);
            Assert.Equal(358, game.GdkCalls.Sum(call => call.Sites));
        }
    }

    [GdkDumpFact]
    public void The_http_client_shows_the_known_gdk_thunks_and_each_report_section()
    {
        var analysis = Analyze(HttpClient, GdkDumpFactAttribute.Library);

        Assert.Equal(HttpClientThunks, analysis.GdkThunkCount);
        Assert.InRange(analysis.GdkNamedCount, 1, HttpClientThunks);
        Assert.Contains(analysis.Sections, section => section.Console);
        Assert.NotEmpty(analysis.Imports);
        Assert.NotEmpty(analysis.DayOneGaps);
        AssertReportsRender(analysis);

        if (analysis.LoaderClassified)
        {
            Assert.Contains(analysis.GdkCalls, call => call.Shimmed == true);
        }
    }

    [DumpFact]
    public void Every_package_dll_analyzes_without_error()
    {
        foreach (var dll in PackageDlls)
        {
            var analysis = Analyze(dll, null);

            Assert.NotEmpty(analysis.Sections);
            AssertReportsRender(analysis);
        }
    }

    private static void AssertReportsRender(ExeAnalysis analysis)
    {
        Assert.False(string.IsNullOrWhiteSpace(ReportMarkdown.Build(analysis)));
        Assert.StartsWith("{", ReportJson.Build(analysis).TrimStart());
    }

    private static ExeAnalysis Analyze(string dll, string? gdkLib)
    {
        var exe = Path.Combine(DumpFactAttribute.Folder!, dll);
        var outDir = Path.Combine(Path.GetTempPath(), "fivestar-exe-tests", Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(outDir);
        var options = new AnalyzeOptions(exe, DumpFactAttribute.Folder, gdkLib, outDir, null);

        return ExeAnalyzer.Analyze(options);
    }
}
