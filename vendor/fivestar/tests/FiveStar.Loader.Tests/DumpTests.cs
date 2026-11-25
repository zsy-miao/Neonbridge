// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Calls;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Loading;
using FiveStar.Loader.Tests.Support;
using Xunit;

namespace FiveStar.Loader.Tests;

/// <summary>The loader on the six Xbox DLLs of the user's dump (FIVESTAR_DUMP).</summary>
public sealed unsafe class DumpTests : IClassFixture<DumpFixture>
{
    private readonly DumpFixture _dump;

    public DumpTests(DumpFixture dump)
    {
        _dump = dump;
    }

    [DumpFact]
    public void The_six_xbox_dlls_and_their_runtime_load_with_every_import_resolved()
    {
        var images = _dump.Runtime.Images.All;
        var imports = images.SelectMany(i => i.Imports).ToList();

        Assert.Equal(9, images.Count);
        Assert.Equal(825, imports.Count);
        Assert.DoesNotContain(imports, i => i.Source == ResolutionSource.Unresolved);
        Assert.All(images, i => Assert.Equal(1, i.EntryResult));
    }

    [DumpFact]
    public void The_http_libraries_reach_winhttp_connections_only_through_the_seal()
    {
        string[] sealedCalls = ["WinHttpConnect", "WinHttpOpenRequest", "WinHttpSendRequest", "WinHttpReceiveResponse", "WinHttpReadData", "WinHttpWriteData", "WinHttpQueryDataAvailable"];
        var transport = _dump.Targets
            .Where(i => i.Name is "libhttpclient.gdk.dll" or "xcurl.dll")
            .SelectMany(i => i.Imports)
            .Where(i => i.Module.Equals("WINHTTP.dll", StringComparison.OrdinalIgnoreCase) && sealedCalls.Contains(i.Symbol))
            .ToList();

        Assert.Equal(2 * sealedCalls.Length, transport.Count);
        Assert.All(transport, i => Assert.Equal(ResolutionSource.Shim, i.Source));
    }

    [DumpFact]
    public void The_inspected_exports_behave_as_their_disassembly_predicts()
    {
        var caller = new ExportCaller(_dump.Runtime, new LoaderLog(TextWriter.Null, null, false));

        var outcomes = caller.Run("all");

        Assert.NotNull(outcomes);
        Assert.Equal(9, outcomes.Count);
        Assert.All(outcomes, o => Assert.True(o.Result is not null && o.Result.AsExpected, $"{o.Alias}: {o.Result?.Summary ?? o.Error}"));
    }

    [DumpFact]
    public void The_package_identity_comes_from_MicrosoftGame_config_next_to_the_main_image()
    {
        var test = _dump.Test!;
        byte* id = stackalloc byte[64];
        byte* path = stackalloc byte[520];
        uint titleId = 0;
        nint mount = 0;

        int hrId = ((delegate* unmanaged<nuint, byte*, int>)test.Gdk("XPackageGetCurrentProcessPackageIdentifier"))(64, id);
        int hrTitle = ((delegate* unmanaged<uint*, int>)test.Gdk("XGameGetXboxTitleId"))(&titleId);
        int hrMount = ((delegate* unmanaged<byte*, nint*, int>)test.Gdk("XPackageMount"))(id, &mount);
        ((delegate* unmanaged<nint, nuint, byte*, int>)test.Gdk("XPackageGetMountPath"))(mount, 520, path);
        string identifier = Marshal.PtrToStringUTF8((nint)id)!;

        Assert.Equal(0, hrId);
        Assert.InRange(identifier.Length, 1, 32);
        Assert.Equal(0, hrTitle);
        Assert.NotEqual(0u, titleId);
        Assert.Equal(0, hrMount);
        Assert.Equal(Path.GetFullPath(_dump.Folder), Marshal.PtrToStringUTF8((nint)path));
    }
}
