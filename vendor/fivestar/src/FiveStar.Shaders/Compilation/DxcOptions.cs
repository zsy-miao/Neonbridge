// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Compilation;

/// <summary>
/// How dxc is run: its path, the time limit of a normal compile, of the -Od recompile after a timeout (longer in a
/// retry run) and of a mesh fallback compile, and the source length above which -Od is used from the start (dxc's
/// optimizer needs minutes on the largest programs; the driver optimizes the DXIL anyway).
/// </summary>
public sealed record DxcOptions(
    string Path,
    TimeSpan CompileTimeout,
    TimeSpan RecompileTimeout,
    TimeSpan FallbackTimeout,
    int LargeSourceLength)
{
    private const int DefaultCompileSeconds = 180;
    private const int DefaultRecompileSeconds = 300;
    private const int RetryRecompileSeconds = 900;
    private const int DefaultFallbackSeconds = 600;
    private const int DefaultLargeSourceLength = 2_500_000;

    /// <summary>The default limits for a dxc at <paramref name="path"/>; a retry run allows a longer recompile.</summary>
    public static DxcOptions Defaults(string path, bool retry)
    {
        var recompile = retry ? RetryRecompileSeconds : DefaultRecompileSeconds;

        return new DxcOptions(
            path,
            TimeSpan.FromSeconds(DefaultCompileSeconds),
            TimeSpan.FromSeconds(recompile),
            TimeSpan.FromSeconds(DefaultFallbackSeconds),
            DefaultLargeSourceLength);
    }
}
