// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Shims.D3D12X.DebugLayer;

/// <summary>
/// D3D12_INFO_QUEUE_FILTER (d3d12sdklayers.h): an allow list, then a deny list, each D3D12_INFO_QUEUE_FILTER_DESC
/// (category, severity and message-ID arrays with their counts). Only the deny list's IDs are used here.
/// </summary>
[StructLayout(LayoutKind.Sequential)]
internal unsafe struct InfoQueueFilter
{
    public uint AllowCategoryCount;
    public int* AllowCategories;
    public uint AllowSeverityCount;
    public int* AllowSeverities;
    public uint AllowIdCount;
    public int* AllowIds;
    public uint DenyCategoryCount;
    public int* DenyCategories;
    public uint DenySeverityCount;
    public int* DenySeverities;
    public uint DenyIdCount;
    public int* DenyIds;
}
