// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Formats.Platform;
using FiveStar.Formats.Tests.Support;
using Xunit;

namespace FiveStar.Formats.Tests;

/// <summary>The exe-analysis readers on a hand-built PE: import call sites, the IAT RVA and the TLS callbacks.</summary>
public sealed class ExeScanTests
{
    [Fact]
    public void Call_sites_count_the_call_and_the_jmp_through_the_import_slot()
    {
        var image = new PeImage(SyntheticPe.Build());
        var imports = PeImportReader.Imports(image);

        var sites = ImportCallSites.Count(image, imports);
        var testFunc = sites.Single(site => site.Function == "TestFunc");

        Assert.Equal("TEST.dll", testFunc.Dll);
        Assert.Equal(SyntheticPe.IatRva, testFunc.SlotRva);
        Assert.Equal(2, testFunc.Sites);
    }

    [Fact]
    public void The_import_keeps_its_address_table_rva()
    {
        var image = new PeImage(SyntheticPe.Build());

        var import = PeImportReader.Imports(image).Single();

        Assert.Equal("TEST.dll", import.Dll);
        Assert.Equal(SyntheticPe.IatRva, import.FirstThunkRva);
    }

    [Fact]
    public void Tls_callbacks_are_read_as_rvas()
    {
        var image = new PeImage(SyntheticPe.Build());

        var callbacks = PeTls.Callbacks(image);

        Assert.Equal([SyntheticPe.TlsCallbackRva], callbacks);
    }
}
