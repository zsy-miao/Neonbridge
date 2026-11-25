// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims;

/// <summary>
/// The one static handle of the loader. Shim entry points are [UnmanagedCallersOnly] static methods whose addresses
/// are written into the IATs of mapped images and handed to native callbacks; such a method cannot carry an instance,
/// so each one resolves the active runtime here and calls into the services it owns. All state lives in that runtime.
/// </summary>
internal static class ShimContext
{
    private static LoaderRuntime? _runtime;
    private static Diagnostics.LoaderLog? _log;

    /// <summary>The runtime whose images are mapped; the most recently created one.</summary>
    public static LoaderRuntime Runtime => Volatile.Read(ref _runtime) ?? throw new InvalidOperationException("no loader runtime is active");

    /// <summary>The active runtime, or null in a mode without a game (warming pipelines, tests).</summary>
    public static LoaderRuntime? ActiveRuntime => Volatile.Read(ref _runtime);

    /// <summary>
    /// The log of the active runtime, else the one a mode without a game set (<see cref="Host.WarmPipelinesCommand"/>):
    /// pipeline builds log through it in both. The warm-up ended on the first [budget] line it wrote while it asked
    /// <see cref="Runtime"/>, which has no runtime there.
    /// </summary>
    public static Diagnostics.LoaderLog? Log => Volatile.Read(ref _runtime)?.Log ?? Volatile.Read(ref _log);

    /// <summary>Sets the log of a mode without a runtime.</summary>
    public static void UseLog(Diagnostics.LoaderLog log)
    {
        Volatile.Write(ref _log, log);
    }

    public static void Activate(LoaderRuntime runtime)
    {
        Volatile.Write(ref _runtime, runtime);
    }

    /// <summary>
    /// Clears <paramref name="runtime"/> as the active one when it still is: logging through a disposed runtime wrote to
    /// its closed writer (tests whose runtimes overlap: MeshWaveProbeTests, RayTracingProgramsTests).
    /// </summary>
    public static void Deactivate(LoaderRuntime runtime)
    {
        Interlocked.CompareExchange(ref _runtime, null, runtime);
    }
}
