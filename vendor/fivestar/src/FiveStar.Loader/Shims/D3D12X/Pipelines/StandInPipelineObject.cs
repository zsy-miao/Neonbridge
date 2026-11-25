// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>
/// The pipeline state handed to the game while the driver still builds the real one (<see cref="StandInPipelines"/>):
/// its own vtable, the desktop pipeline once it exists (0 before), the game-read area, a reference count and the
/// owner's GC handle. The game copies +0x10 to +0x2F of a compute pipeline into its command list (0x14248c700), so
/// those bytes exist and stay zero.
/// </summary>
[StructLayout(LayoutKind.Sequential)]
internal unsafe struct StandInPipelineObject
{
    /// <summary>Bytes the game reads past the vtable and the desktop pointer.</summary>
    public const int GameReadBytes = 0x20;

    public nint Vtable;
    public nint Desktop;
    public fixed byte GameRead[GameReadBytes];
    public int References;
    /// <summary>1 while the build waits for the first use (<see cref="StandInPipelines.Deferred"/>), else 0.</summary>
    public int Deferred;
    /// <summary>1 once a command list has set the stand-in: its work goes before speculative work (<see cref="StandInPipelines.Desktop"/>).</summary>
    public int Used;
    public nint Owner;
}
