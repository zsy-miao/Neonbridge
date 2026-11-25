// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.D3D12X.DebugLayer;

/// <summary>
/// Device Removed Extended Data after a removal: per command list, the auto-breadcrumb operations around the last one
/// the GPU completed, and the page-fault address with the allocations at and near it (d3d12.h layouts).
/// </summary>
internal static unsafe class DredReport
{
    /// <summary>ID3D12DeviceRemovedExtendedData slots: GetAutoBreadcrumbsOutput, GetPageFaultAllocationOutput.</summary>
    private const int BreadcrumbsSlot = 3;
    private const int PageFaultSlot = 4;

    /// <summary>D3D12_AUTO_BREADCRUMB_NODE fields.</summary>
    private const int ListNameOffset = 0x00;
    private const int QueueNameOffset = 0x10;
    private const int ListOffset = 0x20;
    private const int QueueOffset = 0x28;
    private const int CountOffset = 0x30;
    private const int LastValueOffset = 0x38;
    private const int HistoryOffset = 0x40;
    private const int NextNodeOffset = 0x48;

    /// <summary>D3D12_DRED_PAGE_FAULT_OUTPUT and D3D12_DRED_ALLOCATION_NODE fields.</summary>
    private const int PageFaultSize = 0x18;
    private const int ExistingOffset = 0x08;
    private const int FreedOffset = 0x10;
    private const int AllocationTypeOffset = 0x10;
    private const int NextAllocationOffset = 0x18;

    /// <summary>Limits on what one report logs.</summary>
    private const int MaxNodes = 64;
    private const int OpsAroundLast = 4;

    private static readonly Guid DredIid = new("98931d33-5ae8-4791-aa3c-1a73a2934e71");

    /// <summary>The first D3D12_AUTO_BREADCRUMB_OP values (d3d12.h).</summary>
    private static readonly string[] OpNames =
    [
        "SetMarker", "BeginEvent", "EndEvent", "DrawInstanced", "DrawIndexedInstanced", "ExecuteIndirect",
        "Dispatch", "CopyBufferRegion", "CopyTextureRegion", "CopyResource", "CopyTiles", "ResolveSubresource",
        "ClearRenderTargetView", "ClearUnorderedAccessView", "ClearDepthStencilView", "ResourceBarrier",
        "ExecuteBundle", "Present", "ResolveQueryData", "BeginSubmission", "EndSubmission",
    ];

    public static void Write(nint device, LoaderLog log)
    {
        Guid iid = DredIid;
        nint dred = 0;
        int hr = ((delegate* unmanaged<nint, Guid*, nint*, int>)ComObject.Slot(device, ComObject.QueryInterfaceSlot))(device, &iid, &dred);

        if (HResult.Failed(hr))
        {
            log.Warn($"[d3d12-debug] no DRED on this device: {HResult.Format(hr)}");

            return;
        }

        nint head = 0;
        hr = ((delegate* unmanaged<nint, nint*, int>)ComObject.Slot(dred, BreadcrumbsSlot))(dred, &head);
        log.Error($"[d3d12-debug] DRED auto-breadcrumbs ({HResult.Format(hr)}):");

        if (!HResult.Failed(hr))
        {
            WriteBreadcrumbs(head, log);
        }

        byte* fault = stackalloc byte[PageFaultSize];
        hr = ((delegate* unmanaged<nint, byte*, int>)ComObject.Slot(dred, PageFaultSlot))(dred, fault);

        if (!HResult.Failed(hr))
        {
            log.Error($"[d3d12-debug] DRED page fault at GPU VA 0x{*(ulong*)fault:x}");
            WriteAllocations("existing", *(nint*)(fault + ExistingOffset), log);
            WriteAllocations("recently freed", *(nint*)(fault + FreedOffset), log);
        }

        ComObject.Release(dred);
    }

    private static void WriteBreadcrumbs(nint node, LoaderLog log)
    {
        for (int i = 0; node != 0 && i < MaxNodes; i++, node = *(nint*)(node + NextNodeOffset))
        {
            uint count = *(uint*)(node + CountOffset);
            uint* last = *(uint**)(node + LastValueOffset);
            uint completed = last == null ? 0 : *last;
            string queue = Name(node + QueueNameOffset);
            string list = Name(node + ListNameOffset);
            log.Error($"    queue 0x{*(nint*)(node + QueueOffset):x} {queue} list 0x{*(nint*)(node + ListOffset):x} {list}: {completed} of {count} ops done");

            if (completed >= count)
            {
                continue;
            }

            int* history = *(int**)(node + HistoryOffset);
            uint from = completed > OpsAroundLast ? completed - OpsAroundLast : 0;
            uint to = Math.Min(count, completed + OpsAroundLast);

            for (uint op = from; op < to && history != null; op++)
            {
                string marker = op == completed ? " <- first not completed" : string.Empty;
                log.Error($"      op {op}: {BreadcrumbOp(history[op])}{marker}");
            }
        }
    }

    private static void WriteAllocations(string title, nint node, LoaderLog log)
    {
        for (int i = 0; node != 0 && i < MaxNodes; i++, node = *(nint*)(node + NextAllocationOffset))
        {
            log.Error($"    {title}: {Name(node)} type {*(int*)(node + AllocationTypeOffset)}");
        }
    }

    /// <summary>An ANSI name pointer followed by a wide one; either may be null.</summary>
    private static string Name(nint names)
    {
        nint ansi = *(nint*)names;
        nint wide = *(nint*)(names + sizeof(nint));

        if (ansi != 0)
        {
            return Marshal.PtrToStringAnsi(ansi) ?? string.Empty;
        }

        if (wide != 0)
        {
            return Marshal.PtrToStringUni(wide) ?? string.Empty;
        }

        return "(unnamed)";
    }

    /// <summary>A D3D12_AUTO_BREADCRUMB_OP by name, or its number past <see cref="OpNames"/>.</summary>
    private static string BreadcrumbOp(int op)
    {
        if (op >= 0 && op < OpNames.Length)
        {
            return OpNames[op];
        }

        return $"op {op}";
    }
}
