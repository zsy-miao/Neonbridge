// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>One driver compile being measured (<see cref="CompileBudget.Measure"/>): its DXIL, the private bytes at its start and their peak since.</summary>
internal sealed class BuildMeasure(long shaderBytes, long startBytes, int running, long started)
{
    private long _peak = startBytes;

    public long ShaderBytes { get; } = shaderBytes;

    public long StartBytes { get; } = startBytes;

    /// <summary>Compiles running when this one started (itself included).</summary>
    public int Running { get; } = running;

    public long Started { get; } = started;

    public long Peak => Interlocked.Read(ref _peak);

    public void Observe(long bytes)
    {
        long peak = Interlocked.Read(ref _peak);

        while (bytes > peak)
        {
            long seen = Interlocked.CompareExchange(ref _peak, bytes, peak);

            if (seen == peak)
            {
                return;
            }

            peak = seen;
        }
    }
}
