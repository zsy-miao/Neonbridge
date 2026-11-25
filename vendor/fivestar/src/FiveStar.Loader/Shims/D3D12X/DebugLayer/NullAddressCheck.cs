// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Shims.D3D12X.Binding;
using Vortice.Direct3D12;

namespace FiveStar.Loader.Shims.D3D12X.DebugLayer;

/// <summary>
/// <c>--check-null-addresses</c>: at record time, the desktop GPU addresses the shim hands the driver are checked, and
/// an operation that would reach a null or low address, or read past its buffer, is logged (the first
/// <see cref="LoggedPerKind"/> of each <see cref="NullAddressKind"/>, with the list and pipeline) and dropped. A page
/// fault at GPU VA 0 names no operation (DRED gives none); this names the one that would have faulted. The list's queue
/// is known only at submission: its breadcrumbs (<c>--d3d12-breadcrumbs</c>) name it.
/// </summary>
internal static class NullAddressCheck
{
    /// <summary>Addresses below this are no desktop allocation (the first 64 KiB are never mapped).</summary>
    public const ulong AddressFloor = 0x10000;

    /// <summary>Operations logged per kind; the rest are dropped and counted.</summary>
    public const int LoggedPerKind = 8;

    private static readonly long[] Counts = new long[Enum.GetValues<NullAddressKind>().Length];

    public static bool Enabled { get; set; }

    /// <summary>Whether <paramref name="address"/> is null or below <see cref="AddressFloor"/>.</summary>
    public static bool Suspect(ulong address)
    {
        return address < AddressFloor;
    }

    /// <summary>Whether <paramref name="bytes"/> at <paramref name="offset"/> run past a buffer of <paramref name="width"/> bytes.</summary>
    public static bool PastEnd(ulong offset, ulong bytes, ulong width)
    {
        return offset > width || bytes > width - offset;
    }

    /// <summary>The problem with an indirect argument (and count) buffer, null when there is none.</summary>
    public static string? Indirect(nint arguments, ulong argumentOffset, ulong argumentBytes, nint count, ulong countOffset)
    {
        if (arguments == 0)
        {
            return "no desktop argument buffer";
        }

        if (Suspect(DesktopCalls.GpuAddress(arguments) + argumentOffset))
        {
            return $"argument address 0x{DesktopCalls.GpuAddress(arguments) + argumentOffset:x}";
        }

        ulong width = Width(arguments);

        if (PastEnd(argumentOffset, argumentBytes, width))
        {
            return $"arguments 0x{argumentOffset:x}+0x{argumentBytes:x} past the buffer's 0x{width:x} bytes";
        }

        if (count == 0)
        {
            return null;
        }

        ulong countWidth = Width(count);

        if (PastEnd(countOffset, sizeof(uint), countWidth))
        {
            return $"count at 0x{countOffset:x} past the buffer's 0x{countWidth:x} bytes";
        }

        return null;
    }

    /// <summary>The problem with a buffer copy between desktop buffers (a side missing or past its end), null when there is none.</summary>
    public static string? Copy(nint destination, ulong destinationOffset, nint source, ulong sourceOffset, ulong bytes)
    {
        if (destination == 0)
        {
            return $"buffer copy of 0x{bytes:x} bytes with no desktop destination";
        }

        if (source == 0)
        {
            return $"buffer copy of 0x{bytes:x} bytes with no desktop source";
        }

        ulong destinationWidth = Width(destination);
        ulong sourceWidth = Width(source);

        if (PastEnd(destinationOffset, bytes, destinationWidth) || PastEnd(sourceOffset, bytes, sourceWidth))
        {
            return $"buffer copy of 0x{bytes:x} bytes from 0x{sourceOffset:x} of 0x{sourceWidth:x} to 0x{destinationOffset:x} of 0x{destinationWidth:x}";
        }

        return null;
    }

    /// <summary>Whether the user-data ring <paramref name="address"/> is to be dropped (logged).</summary>
    public static unsafe bool DropsRing(XboxObject* list, ulong address)
    {
        return Enabled && Suspect(address) && Drop(NullAddressKind.RootDescriptor, list, $"user-data ring address 0x{address:x}");
    }

    /// <summary>Whether a bottom-of-pipe write of console <paramref name="address"/> at <paramref name="desktop"/> is to be dropped (logged).</summary>
    public static unsafe bool DropsWrite(XboxObject* list, ulong address, ulong desktop)
    {
        return Enabled && Suspect(desktop) && Drop(NullAddressKind.OwnWrite, list, $"bottom-of-pipe write of 0x{address:x} at desktop 0x{desktop:x}");
    }

    /// <summary>Whether a buffer copy between desktop buffers is to be dropped (logged), <see cref="Copy"/>.</summary>
    public static unsafe bool DropsCopy(XboxObject* list, nint destination, ulong destinationOffset, nint source, ulong sourceOffset, ulong bytes)
    {
        if (!Enabled)
        {
            return false;
        }

        string? problem = Copy(destination, destinationOffset, source, sourceOffset, bytes);

        return problem is not null && Drop(NullAddressKind.OwnWrite, list, problem);
    }

    /// <summary>Counts a dropped operation of <paramref name="kind"/> and logs the first ones; true, for the caller to drop it.</summary>
    public static unsafe bool Drop(NullAddressKind kind, XboxObject* list, string detail)
    {
        long seen = Interlocked.Increment(ref Counts[(int)kind]);

        if (seen <= LoggedPerKind)
        {
            nint pipeline = RectangleListState.Of(list)->Pipeline;
            ShimContext.Runtime.D3D12X.Warnings.Warn($"null-address-{kind}-{seen}", $"[null address] {kind} #{seen} dropped: {detail}; list 0x{(nint)list:x}, pipeline {QueueProgress.PipelineName(pipeline)}");
        }

        return true;
    }

    private static ulong Width(nint resource)
    {
        Marshal.AddRef(resource);
        using var desktop = new ID3D12Resource(resource);

        return desktop.Description.Width;
    }
}
