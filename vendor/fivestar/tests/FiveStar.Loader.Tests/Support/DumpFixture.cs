// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Host;
using FiveStar.Loader.Loading;

namespace FiveStar.Loader.Tests.Support;

/// <summary>
/// The six Xbox DLLs of the dump, loaded once with the shipped MSVC runtime mapped beside them, the way the host loads
/// them. Loads nothing when FIVESTAR_DUMP is not set.
/// </summary>
public sealed class DumpFixture : IDisposable
{
    private static readonly string[] XboxDlls =
    [
        "XboxBvhBuilder.dll",
        "Bink2Scarlett.dll",
        "amd_fidelityfx_loader_xbox.dll",
        "amd_fidelityfx_upscaler_xbox.dll",
        "libHttpClient.GDK.dll",
        "XCurl.dll",
    ];

    public DumpFixture()
    {
        string? folder = DumpFactAttribute.Folder;

        if (folder is null)
        {
            return;
        }

        Folder = folder;
        Test = TestRuntime.Create(HostComposition.ShippedCrt);

        foreach (string dll in XboxDlls)
        {
            Targets.Add(Test.Runtime.Loader.Load(Path.Combine(folder, dll), true));
        }
    }

    public string Folder { get; } = string.Empty;

    internal TestRuntime? Test { get; }

    internal List<LoadedImage> Targets { get; } = [];

    internal LoaderRuntime Runtime => Test!.Runtime;

    public void Dispose()
    {
        Test?.Dispose();
    }
}
