// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X.DebugLayer;

/// <summary>Commands other than draws and dispatches that <see cref="GpuBreadcrumbs"/> marks, kept in place of a pipeline (small values no pipeline has).</summary>
internal enum RecordedCommand
{
    None,
    CopyBuffer,
    ResourceBarrier,
    EnhancedBarrier,
}
