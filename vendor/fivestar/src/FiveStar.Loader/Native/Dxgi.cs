// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Native;

/// <summary>
/// The few DXGI calls the display shim makes, through raw vtable slots (dxgi.h): the factory, its adapters and their
/// outputs, and IDXGIOutput6::GetDesc1 for the color space and luminance of a display.
/// </summary>
internal static unsafe class Dxgi
{
    /// <summary>IDXGIFactory1::EnumAdapters1.</summary>
    public const int EnumAdapters1Slot = 12;

    /// <summary>IDXGIAdapter::EnumOutputs.</summary>
    public const int EnumOutputsSlot = 7;

    /// <summary>IDXGIOutput6::GetDesc1.</summary>
    public const int GetDesc1Slot = 27;

    // DXGI_OUTPUT_DESC1: DeviceName[32], RECT, BOOL, rotation, HMONITOR, BitsPerColor, ColorSpace, four primaries,
    // MinLuminance, MaxLuminance, MaxFullFrameLuminance.
    public const int OutputDesc1Size = 152;
    public const int ColorSpaceOffset = 100;
    public const int MinLuminanceOffset = 136;
    public const int MaxLuminanceOffset = 140;
    public const int MaxFullFrameLuminanceOffset = 144;

    /// <summary>DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020: the desktop runs in HDR10.</summary>
    public const uint ColorSpaceHdr10 = 12;

    /// <summary>IDXGIAdapter3::QueryVideoMemoryInfo(node, DXGI_MEMORY_SEGMENT_GROUP, DXGI_QUERY_VIDEO_MEMORY_INFO*).</summary>
    public const int QueryVideoMemoryInfoSlot = 14;

    public static readonly Guid Factory1Id = new("770aae78-f26f-4dba-a829-253c83d1b387");
    public static readonly Guid Adapter3Id = new("645967a4-1392-4310-a798-8053ce3e93fd");
    public static readonly Guid Output6Id = new("068346e8-aaec-4b84-add7-137f513f77a1");

    [DllImport("dxgi")]
    public static extern int CreateDXGIFactory1(Guid* riid, nint* factory);
}
