// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Formats.Platform;
using FiveStar.Formats.Tests.Support;
using Xunit;

namespace FiveStar.Formats.Tests;

/// <summary>The GDK thunk ABI table: the PC library names the console thunks in the shipped libHttpClient.</summary>
public sealed class GdkAbiTests
{
    [GdkFact]
    public void The_pc_library_names_most_thunks_of_the_shipped_http_client()
    {
        var library = GdkThunkLibrary.Read(File.ReadAllBytes(GdkFactAttribute.Library!));
        var module = new PeImage(File.ReadAllBytes(Path.Combine(DumpFactAttribute.Folder!, "libHttpClient.GDK.dll")));

        var found = GdkThunkScanner.Scan(module, library.Select(thunk => thunk.Clsid).ToHashSet());
        var coverage = GdkAbiTable.Coverage("libHttpClient.GDK.dll", library, found);

        Assert.Equal(418, library.Count);
        // The 43 without a name are console-only: XUser through a console interface id, and four single functions.
        Assert.Equal(408, coverage.Thunks);
        Assert.Equal(365, coverage.Thunks - coverage.Unnamed.Count);
    }
}
