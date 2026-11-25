// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Shims.D3D12X.Present;

/// <summary>
/// D3D12_RESOURCE_BARRIER (d3d12.h): made here as a TRANSITION of all subresources; the ALIASING and UAV forms share
/// the layout (aliasing after-resource at +16, UAV resource at +8).
/// </summary>
[StructLayout(LayoutKind.Explicit, Size = 32)]
internal struct TransitionBarrier
{
    /// <summary>D3D12_RESOURCE_BARRIER_TYPE values.</summary>
    public const int TransitionType = 0;
    public const int AliasingType = 1;
    public const int UavType = 2;

    /// <summary>D3D12_RESOURCE_BARRIER_FLAG_BEGIN_ONLY: the first half of a split barrier.</summary>
    public const int BeginOnlyFlag = 0x1;

    /// <summary>D3D12_RESOURCE_STATE_PRESENT (= COMMON), COPY_DEST and RENDER_TARGET.</summary>
    public const int PresentState = 0;
    public const int CopyDestState = 0x400;
    public const int RenderTargetState = 0x4;

    /// <summary>D3D12_RESOURCE_STATE_COPY_SOURCE.</summary>
    public const int CopySourceState = 0x800;

    /// <summary>D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES.</summary>
    private const uint AllSubresources = 0xFFFFFFFF;

    [FieldOffset(0)]
    public int Type;

    [FieldOffset(4)]
    public int Flags;

    [FieldOffset(8)]
    public nint Resource;

    [FieldOffset(16)]
    public uint Subresource;

    [FieldOffset(16)]
    public nint AliasingAfter;

    [FieldOffset(20)]
    public int StateBefore;

    [FieldOffset(24)]
    public int StateAfter;

    [HotPath]
    public static TransitionBarrier Of(nint resource, int before, int after)
    {
        return new TransitionBarrier
        {
            Resource = resource,
            Subresource = AllSubresources,
            StateBefore = before,
            StateAfter = after,
        };
    }
}
