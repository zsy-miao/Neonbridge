// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Loading;
using FiveStar.Loader.Shims.D3D12X.Pipelines;
using FiveStar.Loader.Shims.Gdk;
using FiveStar.Loader.Shims.Network;

namespace FiveStar.Benchmarks;

/// <summary>A loader runtime with no images, a silent log and the temp folder for saves, as the benchmarks need it.</summary>
internal static class BenchmarkRuntime
{
    /// <summary>What XSystemGetDeviceType returns here (Series X).</summary>
    public const uint DeviceType = 7;

    public static LoaderRuntime Create()
    {
        var log = new LoaderLog(TextWriter.Null, null, false);
        var options = new LoaderOptions([], new HashSet<string>(), UnresolvedMode.Stub, false, Path.GetTempPath(), null, 0);
        var gdk = new GdkOptions(null, Path.GetTempPath(), Path.GetTempPath(), Path.GetTempPath(), DeviceType, new Version(), NetworkOptions.Offline);
        var environment = new LoaderEnvironment(string.Empty, TimeProvider.System, IgnoreFailure);

        return new LoaderRuntime(options, gdk, new PipelineFolders(null, null, null), environment, log);
    }

    /// <summary>The native entry point of a GDK shim.</summary>
    public static nint Gdk(LoaderRuntime runtime, string name)
    {
        return runtime.Shims.TryResolve(GdkModule.Name, name, 0)!.Address;
    }

    /// <summary>No benchmark reaches an unresolved import in trap mode.</summary>
    private static void IgnoreFailure(string message)
    {
    }
}
