// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.D3D12X.Present;

/// <summary>
/// The present and frame-pacing slots: queue slot 32 (PresentX) and device slots 78 to 80 (SetFrameIntervalX,
/// ScheduleFrameEventX, WaitFrameEventX; names *inferred* from the arguments, docs/d3d12x-shim.md).
/// </summary>
internal static unsafe class PresentSlots
{
    public const int PresentSlot = 32;
    public const int SetFrameIntervalSlot = 78;
    public const int ScheduleFrameEventSlot = 79;
    public const int WaitFrameEventSlot = 80;

    /// <summary>Binds the four slots on the device and queue vtables.</summary>
    public static void Bind(XboxObjects objects)
    {
        delegate* unmanaged<XboxObject*, uint, PresentPlane*, uint*, int> present = &PresentX;
        delegate* unmanaged<XboxObject*, nint, uint, uint, uint, int> interval = &SetFrameInterval;
        delegate* unmanaged<XboxObject*, uint, uint, nint, uint, int> schedule = &ScheduleFrameEvent;
        delegate* unmanaged<XboxObject*, uint, uint, nint, uint, ulong*, int> wait = &WaitFrameEvent;

        objects.SetHandler(XboxInterfaceKind.CommandQueue, PresentSlot, (nint)present);
        objects.SetHandler(XboxInterfaceKind.Device, SetFrameIntervalSlot, (nint)interval);
        objects.SetHandler(XboxInterfaceKind.Device, ScheduleFrameEventSlot, (nint)schedule);
        objects.SetHandler(XboxInterfaceKind.Device, WaitFrameEventSlot, (nint)wait);
    }

    /// <summary>Queue slot 32: (plane count, planes, parameters).</summary>
    [HotPath]
    [UnmanagedCallersOnly]
    public static int PresentX(XboxObject* self, uint count, PresentPlane* planes, uint* parameters)
    {
        try
        {
            var d3d12x = ShimContext.Runtime.D3D12X;

            if (count > 0 && planes != null)
            {
                d3d12x.Pacer.Presented(planes->Token);
            }

            // A memory wait is not kept across a present (as victory-gta6 forces its held work there): the game's own
            // frame wait (0x142474f80) can stand behind it while the writer is next frame's work (a 5 s stall, then a
            // removed device, at the first story-load frame).
            MemoryWaits.ReleaseAll();
            d3d12x.FrameThrottle.Presented();

            return d3d12x.Presenter.Present(self->Inner, count, planes, parameters);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "PresentSlots.PresentX");

            return HResult.Fail;
        }
    }

    /// <summary>
    /// Device slot 78: (null, interval in µs, period, flags 0). The game takes the interval from a table of refresh
    /// multiples (0x1461ece60: 16667, 33333, 50000, 66667 at 60 Hz; half at 120 Hz) at 0x14247abd8.
    /// </summary>
    [UnmanagedCallersOnly]
    public static int SetFrameInterval(XboxObject* self, nint output, uint length, uint period, uint flags)
    {
        try
        {
            ShimContext.Runtime.Log.Detail($"  [d3d12_xs] ID3D12Device.X::Slot78 SetFrameInterval(output 0x{output:x}, length {length}, period {period}, flags 0x{flags:x}) -> S_OK");
            ShimContext.Runtime.D3D12X.Pacer.SetInterval(length, period);

            return HResult.Ok;
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "PresentSlots.SetFrameInterval");

            return HResult.Fail;
        }
    }

    /// <summary>Device slot 79: (event type, offset in µs from the origin, null, flags 0); the game schedules types 0 and 2.</summary>
    [UnmanagedCallersOnly]
    public static int ScheduleFrameEvent(XboxObject* self, uint type, uint offset, nint output, uint flags)
    {
        try
        {
            ShimContext.Runtime.Log.Detail($"  [d3d12_xs] ID3D12Device.X::Slot79 ScheduleFrameEvent(type {type}, offset {offset}, output 0x{output:x}, flags 0x{flags:x}) -> S_OK");
            ShimContext.Runtime.D3D12X.Pacer.Schedule(type, offset);

            return HResult.Ok;
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "PresentSlots.ScheduleFrameEvent");

            return HResult.Fail;
        }
    }

    /// <summary>Device slot 80: (event type, timeout in ms, null parameters, flags, UINT64* token in/out).</summary>
    [HotPath]
    [UnmanagedCallersOnly]
    public static int WaitFrameEvent(XboxObject* self, uint type, uint timeout, nint parameters, uint flags, ulong* token)
    {
        try
        {
            return ShimContext.Runtime.D3D12X.Pacer.Wait(type, timeout, flags, token);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "PresentSlots.WaitFrameEvent");

            return HResult.Fail;
        }
    }
}
