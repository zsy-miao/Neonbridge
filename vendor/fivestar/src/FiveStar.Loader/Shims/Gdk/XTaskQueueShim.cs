// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>
/// XTaskQueue.h entry points; the work is in <see cref="TaskQueueService"/>. Callbacks are
/// <c>void XTaskQueueCallback(void* context, bool canceled)</c>; a registration token is one 64-bit value.
/// </summary>
internal static unsafe partial class XTaskQueueShim
{
    [Shim(GdkModule.Name, "XTaskQueueCreate")]
    [UnmanagedCallersOnly]
    private static int XTaskQueueCreate(uint workMode, uint completionMode, nint* queue)
    {
        try
        {
            var gdk = ShimContext.Runtime.Gdk;

            try
            {
                return gdk.TaskQueues.Create(workMode, completionMode, queue);
            }
            catch (Exception e)
            {
                return gdk.Boundary.Failed(nameof(XTaskQueueCreate), e);
            }
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XTaskQueueShim.XTaskQueueCreate");

            return HResult.Fail;
        }
    }

    [Shim(GdkModule.Name, "XTaskQueueGetPort")]
    [UnmanagedCallersOnly]
    private static int XTaskQueueGetPort(nint queue, uint port, nint* portHandle)
    {
        try
        {
            var gdk = ShimContext.Runtime.Gdk;

            try
            {
                return gdk.TaskQueues.GetPort(queue, port, portHandle);
            }
            catch (Exception e)
            {
                return gdk.Boundary.Failed(nameof(XTaskQueueGetPort), e);
            }
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XTaskQueueShim.XTaskQueueGetPort");

            return HResult.Fail;
        }
    }

    [Shim(GdkModule.Name, "XTaskQueueCreateComposite")]
    [UnmanagedCallersOnly]
    private static int XTaskQueueCreateComposite(nint workPort, nint completionPort, nint* queue)
    {
        try
        {
            var gdk = ShimContext.Runtime.Gdk;

            try
            {
                return gdk.TaskQueues.CreateComposite(workPort, completionPort, queue);
            }
            catch (Exception e)
            {
                return gdk.Boundary.Failed(nameof(XTaskQueueCreateComposite), e);
            }
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XTaskQueueShim.XTaskQueueCreateComposite");

            return HResult.Fail;
        }
    }

    [HotPath]
    [Shim(GdkModule.Name, "XTaskQueueDispatch")]
    [UnmanagedCallersOnly]
    private static byte XTaskQueueDispatch(nint queue, uint port, uint timeoutInMs)
    {
        try
        {
            var gdk = ShimContext.Runtime.Gdk;

            try
            {
                bool dispatched = gdk.TaskQueues.Dispatch(queue, port, timeoutInMs);
                byte result = dispatched ? GdkModule.True : GdkModule.False;

                return result;
            }
            catch (Exception e)
            {
                gdk.Boundary.Report(nameof(XTaskQueueDispatch), e);

                return GdkModule.False;
            }
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XTaskQueueShim.XTaskQueueDispatch");

            return default;
        }
    }

    [HotPath]
    [Shim(GdkModule.Name, "XTaskQueueSubmitCallback")]
    [UnmanagedCallersOnly]
    private static int XTaskQueueSubmitCallback(nint queue, uint port, nint context, nint callback)
    {
        try
        {
            var gdk = ShimContext.Runtime.Gdk;

            try
            {
                return gdk.TaskQueues.Submit(queue, port, 0, context, callback);
            }
            catch (Exception e)
            {
                return gdk.Boundary.Failed(nameof(XTaskQueueSubmitCallback), e);
            }
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XTaskQueueShim.XTaskQueueSubmitCallback");

            return HResult.Fail;
        }
    }

    [HotPath]
    [Shim(GdkModule.Name, "XTaskQueueSubmitDelayedCallback")]
    [UnmanagedCallersOnly]
    private static int XTaskQueueSubmitDelayedCallback(nint queue, uint port, uint delayMs, nint context, nint callback)
    {
        try
        {
            var gdk = ShimContext.Runtime.Gdk;

            try
            {
                return gdk.TaskQueues.Submit(queue, port, delayMs, context, callback);
            }
            catch (Exception e)
            {
                return gdk.Boundary.Failed(nameof(XTaskQueueSubmitDelayedCallback), e);
            }
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XTaskQueueShim.XTaskQueueSubmitDelayedCallback");

            return HResult.Fail;
        }
    }

    [Shim(GdkModule.Name, "XTaskQueueRegisterWaiter")]
    [UnmanagedCallersOnly]
    private static int XTaskQueueRegisterWaiter(nint queue, uint port, nint waitHandle, nint context, nint callback, ulong* token)
    {
        try
        {
            var gdk = ShimContext.Runtime.Gdk;

            try
            {
                return gdk.TaskQueues.RegisterWaiter(queue, port, waitHandle, context, callback, token);
            }
            catch (Exception e)
            {
                return gdk.Boundary.Failed(nameof(XTaskQueueRegisterWaiter), e);
            }
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XTaskQueueShim.XTaskQueueRegisterWaiter");

            return HResult.Fail;
        }
    }

    [Shim(GdkModule.Name, "XTaskQueueUnregisterWaiter")]
    [UnmanagedCallersOnly]
    private static void XTaskQueueUnregisterWaiter(nint queue, ulong token)
    {
        try
        {
            var gdk = ShimContext.Runtime.Gdk;

            try
            {
                gdk.TaskQueues.UnregisterWaiter(queue, token);
            }
            catch (Exception e)
            {
                gdk.Boundary.Report(nameof(XTaskQueueUnregisterWaiter), e);
            }
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XTaskQueueShim.XTaskQueueUnregisterWaiter");
        }
    }

    [Shim(GdkModule.Name, "XTaskQueueRegisterMonitor")]
    [UnmanagedCallersOnly]
    private static int XTaskQueueRegisterMonitor(nint queue, nint context, nint callback, ulong* token)
    {
        try
        {
            var gdk = ShimContext.Runtime.Gdk;

            try
            {
                return gdk.TaskQueues.RegisterMonitor(queue, context, callback, token);
            }
            catch (Exception e)
            {
                return gdk.Boundary.Failed(nameof(XTaskQueueRegisterMonitor), e);
            }
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XTaskQueueShim.XTaskQueueRegisterMonitor");

            return HResult.Fail;
        }
    }

    [Shim(GdkModule.Name, "XTaskQueueUnregisterMonitor")]
    [UnmanagedCallersOnly]
    private static void XTaskQueueUnregisterMonitor(nint queue, ulong token)
    {
        try
        {
            var gdk = ShimContext.Runtime.Gdk;

            try
            {
                gdk.TaskQueues.UnregisterMonitor(queue, token);
            }
            catch (Exception e)
            {
                gdk.Boundary.Report(nameof(XTaskQueueUnregisterMonitor), e);
            }
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XTaskQueueShim.XTaskQueueUnregisterMonitor");
        }
    }

    [Shim(GdkModule.Name, "XTaskQueueDuplicateHandle")]
    [UnmanagedCallersOnly]
    private static int XTaskQueueDuplicateHandle(nint queue, nint* duplicated)
    {
        try
        {
            var gdk = ShimContext.Runtime.Gdk;

            try
            {
                return gdk.TaskQueues.Duplicate(queue, duplicated);
            }
            catch (Exception e)
            {
                return gdk.Boundary.Failed(nameof(XTaskQueueDuplicateHandle), e);
            }
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XTaskQueueShim.XTaskQueueDuplicateHandle");

            return HResult.Fail;
        }
    }

    [Shim(GdkModule.Name, "XTaskQueueCloseHandle")]
    [UnmanagedCallersOnly]
    private static void XTaskQueueCloseHandle(nint queue)
    {
        try
        {
            var gdk = ShimContext.Runtime.Gdk;

            try
            {
                gdk.TaskQueues.Close(queue);
            }
            catch (Exception e)
            {
                gdk.Boundary.Report(nameof(XTaskQueueCloseHandle), e);
            }
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XTaskQueueShim.XTaskQueueCloseHandle");
        }
    }

    [Shim(GdkModule.Name, "XTaskQueueTerminate")]
    [UnmanagedCallersOnly]
    private static int XTaskQueueTerminate(nint queue, byte wait, nint context, nint callback)
    {
        try
        {
            var gdk = ShimContext.Runtime.Gdk;

            try
            {
                return gdk.TaskQueues.Terminate(queue, wait != 0, context, callback);
            }
            catch (Exception e)
            {
                return gdk.Boundary.Failed(nameof(XTaskQueueTerminate), e);
            }
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XTaskQueueShim.XTaskQueueTerminate");

            return HResult.Fail;
        }
    }

    [Shim(GdkModule.Name, "XTaskQueueGetCurrentProcessTaskQueue")]
    [UnmanagedCallersOnly]
    private static byte XTaskQueueGetCurrentProcessTaskQueue(nint* queue)
    {
        try
        {
            var gdk = ShimContext.Runtime.Gdk;

            try
            {
                bool found = gdk.TaskQueues.GetProcessQueue(queue);
                byte result = found ? GdkModule.True : GdkModule.False;

                return result;
            }
            catch (Exception e)
            {
                gdk.Boundary.Report(nameof(XTaskQueueGetCurrentProcessTaskQueue), e);

                return GdkModule.False;
            }
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XTaskQueueShim.XTaskQueueGetCurrentProcessTaskQueue");

            return default;
        }
    }

    [Shim(GdkModule.Name, "XTaskQueueSetCurrentProcessTaskQueue")]
    [UnmanagedCallersOnly]
    private static void XTaskQueueSetCurrentProcessTaskQueue(nint queue)
    {
        try
        {
            var gdk = ShimContext.Runtime.Gdk;

            try
            {
                gdk.TaskQueues.SetProcessQueue(queue);
            }
            catch (Exception e)
            {
                gdk.Boundary.Report(nameof(XTaskQueueSetCurrentProcessTaskQueue), e);
            }
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XTaskQueueShim.XTaskQueueSetCurrentProcessTaskQueue");
        }
    }

    /// <summary>bool XTaskQueueIsEmpty(XTaskQueueHandle queue, XTaskQueuePort port) (newer XTaskQueue.h).</summary>
    [HotPath]
    [Shim(GdkModule.Name, "XTaskQueueIsEmpty")]
    [UnmanagedCallersOnly]
    private static byte XTaskQueueIsEmpty(nint queue, uint port)
    {
        try
        {
            bool empty = ShimContext.Runtime.Gdk.TaskQueues.IsEmpty(queue, port);
            byte result = empty ? GdkModule.True : GdkModule.False;

            return result;
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XTaskQueueShim.XTaskQueueIsEmpty");

            return default;
        }
    }
}
