// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Diagnostics;

/// <summary>How often the loader's own interposed system functions were called by mapped code.</summary>
internal sealed class LoaderCounters
{
    private int _pcToFileHeaderHits;
    private int _moduleShimHits;

    /// <summary>RtlPcToFileHeader calls answered for a mapped image (C++ throws go through it).</summary>
    public int PcToFileHeaderHits => Volatile.Read(ref _pcToFileHeaderHits);

    /// <summary>Module identity and LoadLibrary calls answered by the module shims.</summary>
    public int ModuleShimHits => Volatile.Read(ref _moduleShimHits);

    public void CountPcToFileHeader()
    {
        Interlocked.Increment(ref _pcToFileHeaderHits);
    }

    public void CountModuleShim()
    {
        Interlocked.Increment(ref _moduleShimHits);
    }
}
