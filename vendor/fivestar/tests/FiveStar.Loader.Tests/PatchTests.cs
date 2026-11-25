// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Security.Cryptography;
using FiveStar.Loader.Host;
using FiveStar.Loader.Mods;
using FiveStar.Loader.Tests.Support;
using Xunit;

namespace FiveStar.Loader.Tests;

/// <summary>Code patches on Bink2Scarlett.dll of the dump, whose BinkUtilCPUs is <c>mov eax,7; ret</c>.</summary>
public sealed unsafe class PatchTests
{
    /// <summary>BinkUtilCPUs and the padding and code after it.</summary>
    private const string CpusPattern = "B8 07 00 00 00 C3 CC ?? ?? ?? ?? ?? ?? ?? ?? ?? F7 DA";

    [DumpFact]
    public void A_patch_applies_to_its_build_and_never_to_another_or_to_an_ambiguous_match()
    {
        string dll = Path.Combine(DumpFactAttribute.Folder!, "Bink2Scarlett.dll");
        string hash = Convert.ToHexString(SHA256.HashData(File.ReadAllBytes(dll)));
        string text = $"""
            [twelve-cpus]
            module = Bink2Scarlett.dll
            sha256 = {hash}
            description = BinkUtilCPUs returns 12
            pattern = {CpusPattern}
            offset = 1
            bytes = 0C

            [other-build]
            module = Bink2Scarlett.dll
            sha256 = {new string('0', 64)}
            pattern = {CpusPattern}
            offset = 1
            bytes = 0D

            [ambiguous]
            module = Bink2Scarlett.dll
            pattern = C3 CC CC CC
            bytes = 90 90 90 90
            """;

        using var test = TestRuntime.Create(HostComposition.ShippedCrt);
        test.Runtime.Patcher.Add(PatchFileReader.Parse(text.Split('\n'), "test.fspatch"));
        var image = test.Runtime.Loader.Load(dll, true);
        byte* cpus = (byte*)image.Exports.Find("BinkUtilCPUs", 0)!.Address;
        var statuses = test.Runtime.Patcher.Outcomes.Select(o => o.Status).ToArray();

        Assert.Equal([PatchStatus.Applied, PatchStatus.Skipped, PatchStatus.Failed], statuses);
        Assert.Equal(12, ((delegate* unmanaged<int>)cpus)());
        Assert.Equal(0xC3, cpus[5]);
        Assert.Equal(0xCC, cpus[6]);
    }

    [DumpFact]
    public void A_jump_patch_sends_the_function_to_a_shim()
    {
        string dll = Path.Combine(DumpFactAttribute.Folder!, "Bink2Scarlett.dll");
        string[] text =
        [
            "[cpus-to-title-id]",
            "module = Bink2Scarlett.dll",
            $"pattern = {CpusPattern}",
            "jump = xgameruntime.dll!XGameGetXboxTitleId",
        ];

        using var test = TestRuntime.Create(HostComposition.ShippedCrt);
        test.Runtime.Patcher.Add(PatchFileReader.Parse(text, "test.fspatch"));
        var image = test.Runtime.Loader.Load(dll, true);
        var patched = (delegate* unmanaged<uint*, int>)image.Exports.Find("BinkUtilCPUs", 0)!.Address;
        var shim = (delegate* unmanaged<uint*, int>)test.Gdk("XGameGetXboxTitleId");

        uint viaPatch = 0;
        uint viaShim = 0;

        int hr = patched(&viaPatch);
        _ = shim(&viaShim);

        Assert.Equal(PatchStatus.Applied, test.Runtime.Patcher.Outcomes.Single().Status);
        Assert.Equal(0, hr);
        Assert.NotEqual(0u, viaPatch);
        Assert.Equal(viaShim, viaPatch);
    }
}
