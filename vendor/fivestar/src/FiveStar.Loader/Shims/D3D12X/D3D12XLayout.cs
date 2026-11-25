// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X;

/// <summary>
/// The D3D12.X facts the device shim depends on, as read from the game's calls (docs/d3d12x-shim.md). Slot maps of
/// the wrapped interfaces are in <see cref="XboxInterfaces"/>.
/// </summary>
internal static class D3D12XLayout
{
    public const string Module = "d3d12_xs.dll";

    /// <summary>X-only device slot that creates the compute queues: (desc, REFIID, void**).</summary>
    public const int CreateCommandQueueSlot = 48;

    /// <summary>X-only device slot that creates command lists: (desc, allocator, PSO, REFIID, void**).</summary>
    public const int CreateCommandListSlot = 73;

    /// <summary>Offset of the desktop object pointer in <see cref="XboxObject"/>.</summary>
    public const byte InnerOffset = 8;

    /// <summary>D3D_FEATURE_LEVEL_12_1 (d3dcommon.h): the console GPU's level.</summary>
    public const int MinimumFeatureLevel = 0xC100;

    /// <summary>Bytes of D3D12XBOX_CREATE_DEVICE_PARAMETERS the game clears before filling it.</summary>
    public const int ParametersSize = 0x50;

    /// <summary>The one dword the game sets in the D3D12ConfigureVirtualMemoryX argument (field name unknown).</summary>
    public const int VirtualMemoryCountOffset = 0x04;

    /// <summary>IID_ID3D12Device (d3d12.h).</summary>
    public static readonly Guid DeviceIid = new("189819f1-1db6-4b57-be54-1821339b85f7");

    /// <summary>IID_ID3D12GraphicsCommandList (d3d12.h).</summary>
    public static readonly Guid GraphicsCommandListIid = new("5b160d0f-ac1b-4185-8ba8-b3ae42a5a455");
}
