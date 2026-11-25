// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Host;
using FiveStar.Loader.Tests.Support;
using Xunit;

namespace FiveStar.Loader.Tests;

/// <summary>The classify mode behind <c>fivestar exe analyze</c>: the same shim table verdicts, without mapping an image.</summary>
public sealed class ImportClassifyTests
{
    [Fact]
    public void Imports_classify_as_shim_xbox_stub_or_host()
    {
        using var test = TestRuntime.Create();
        var classifier = new ImportClassifier(test.Runtime.Shims, [], new HashSet<string>());

        var shim = classifier.Classify("kernel32.dll", "GetProcAddress", 0);
        var forward = classifier.Classify("d3d12_xs.dll", "D3D12SerializeRootSignature", 0);
        var xboxOnly = classifier.Classify("d3d12_xs.dll", "SomeConsoleOnlyEntry", 0);
        var host = classifier.Classify("kernel32.dll", "NoSuchFunctionXyz", 0);

        Assert.Equal(ImportClassKind.Shim, shim.Kind);
        Assert.Equal(ImportClassKind.Shim, forward.Kind);
        Assert.Equal(ImportClassKind.XboxStub, xboxOnly.Kind);
        Assert.Equal(ImportClassKind.Host, host.Kind);
    }

    /// <summary>A module neither shimmed, packaged nor on the PC is never reported as the host's.</summary>
    [Fact]
    public void Modules_absent_from_the_host_classify_by_where_they_are()
    {
        using var test = TestRuntime.Create();
        File.WriteAllBytes(Path.Combine(test.Folder, "middleware.dll"), []);
        var classifier = new ImportClassifier(test.Runtime.Shims, [test.Folder], new HashSet<string>());

        var package = classifier.Classify("Middleware.dll", "Start", 0);
        var missing = classifier.Classify("consolehal.dll", "Connect", 0);
        var apiSet = classifier.Classify("api-ms-win-core-synch-l1-1-0.dll", "Sleep", 0);
        var audio = classifier.Classify("chad.dll", "XDspGetStatistics", 0);
        var inputByOrdinal = classifier.Classify("GameInput.dll", null, 1);

        Assert.Equal(ImportClassKind.Shim, inputByOrdinal.Kind);
        Assert.Equal(ImportClassKind.Package, package.Kind);
        Assert.Equal(ImportClassKind.Missing, missing.Kind);
        Assert.Equal(ImportClassKind.Host, apiSet.Kind);
        Assert.Equal(ImportClassKind.XboxStub, audio.Kind);
    }
}
