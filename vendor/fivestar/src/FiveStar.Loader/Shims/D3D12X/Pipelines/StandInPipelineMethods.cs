// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>
/// The ID3D12PipelineState vtable of a <see cref="StandInPipelineObject"/> (d3d12.h order): the reference count is the
/// stand-in's own; the ID3D12Object, GetDevice and GetCachedBlob slots go to the desktop pipeline once it is built and
/// answer as an object without data before.
/// </summary>
internal static unsafe class StandInPipelineMethods
{
    // IUnknown, ID3D12Object, ID3D12DeviceChild and ID3D12PipelineState slots (d3d12.h).
    private const int AddRefSlot = 1;
    private const int GetPrivateDataSlot = 3;
    private const int SetPrivateDataSlot = 4;
    private const int SetPrivateDataInterfaceSlot = 5;
    private const int SetNameSlot = 6;
    private const int GetDeviceSlot = 7;
    private const int GetCachedBlobSlot = 8;
    private const int SlotCount = 9;

    /// <summary>DXGI_ERROR_NOT_FOUND (dxgi.h): GetPrivateData for a GUID without data.</summary>
    private const int NotFound = unchecked((int)0x887A0002);

    /// <summary>The vtable every stand-in points to (allocated once per owner, never freed).</summary>
    public static nint* CreateVtable()
    {
        var vtable = (nint*)NativeMemory.AllocZeroed(SlotCount, (nuint)sizeof(nint));
        vtable[ComObject.QueryInterfaceSlot] = (nint)(delegate* unmanaged<StandInPipelineObject*, Guid*, nint*, int>)&QueryInterface;
        vtable[AddRefSlot] = (nint)(delegate* unmanaged<StandInPipelineObject*, uint>)&AddRef;
        vtable[ComObject.ReleaseSlot] = (nint)(delegate* unmanaged<StandInPipelineObject*, uint>)&Release;

        vtable[GetPrivateDataSlot] = (nint)(delegate* unmanaged<StandInPipelineObject*, Guid*, uint*, nint, int>)&GetPrivateData;
        vtable[SetPrivateDataSlot] = (nint)(delegate* unmanaged<StandInPipelineObject*, Guid*, uint, nint, int>)&SetPrivateData;
        vtable[SetPrivateDataInterfaceSlot] = (nint)(delegate* unmanaged<StandInPipelineObject*, Guid*, nint, int>)&SetPrivateDataInterface;
        vtable[SetNameSlot] = (nint)(delegate* unmanaged<StandInPipelineObject*, char*, int>)&SetName;
        vtable[GetDeviceSlot] = (nint)(delegate* unmanaged<StandInPipelineObject*, Guid*, nint*, int>)&GetDevice;
        vtable[GetCachedBlobSlot] = (nint)(delegate* unmanaged<StandInPipelineObject*, nint*, int>)&GetCachedBlob;

        return vtable;
    }

    [UnmanagedCallersOnly]
    private static uint AddRef(StandInPipelineObject* self)
    {
        try
        {
            return (uint)Interlocked.Increment(ref self->References);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "StandInPipelineMethods.AddRef");

            return default;
        }
    }

    [UnmanagedCallersOnly]
    private static uint Release(StandInPipelineObject* self)
    {
        try
        {
            int left = Interlocked.Decrement(ref self->References);

            if (left == 0 && GCHandle.FromIntPtr(self->Owner).Target is StandInPipelines owner)
            {
                owner.Retire(self);
            }

            return (uint)left;
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "StandInPipelineMethods.Release");

            return default;
        }
    }

    /// <summary>The stand-in answers for every interface the desktop pipeline would (IUnknown and ID3D12PipelineState before it is built).</summary>
    [UnmanagedCallersOnly]
    private static int QueryInterface(StandInPipelineObject* self, Guid* iid, nint* result)
    {
        try
        {
            if (result == null)
            {
                return HResult.Pointer;
            }

            nint desktop = Volatile.Read(ref self->Desktop);
            bool known = *iid == DesktopDevice.PipelineStateIid || *iid == DesktopDevice.UnknownIid;

            if (desktop != 0)
            {
                nint answer = 0;
                known = !HResult.Failed(((delegate* unmanaged<nint, Guid*, nint*, int>)ComObject.Slot(desktop, ComObject.QueryInterfaceSlot))(desktop, iid, &answer));
                ComObject.Release(answer);
            }

            if (!known)
            {
                *result = 0;

                return HResult.NoInterface;
            }

            Interlocked.Increment(ref self->References);
            *result = (nint)self;

            return HResult.Ok;
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "StandInPipelineMethods.QueryInterface");

            return HResult.Fail;
        }
    }

    [UnmanagedCallersOnly]
    private static int GetPrivateData(StandInPipelineObject* self, Guid* guid, uint* size, nint data)
    {
        try
        {
            nint desktop = Volatile.Read(ref self->Desktop);

            if (desktop == 0)
            {
                return NotFound;
            }

            return ((delegate* unmanaged<nint, Guid*, uint*, nint, int>)ComObject.Slot(desktop, GetPrivateDataSlot))(desktop, guid, size, data);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "StandInPipelineMethods.GetPrivateData");

            return HResult.Fail;
        }
    }

    [UnmanagedCallersOnly]
    private static int SetPrivateData(StandInPipelineObject* self, Guid* guid, uint size, nint data)
    {
        try
        {
            nint desktop = Volatile.Read(ref self->Desktop);

            if (desktop == 0)
            {
                return HResult.Ok;
            }

            return ((delegate* unmanaged<nint, Guid*, uint, nint, int>)ComObject.Slot(desktop, SetPrivateDataSlot))(desktop, guid, size, data);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "StandInPipelineMethods.SetPrivateData");

            return HResult.Fail;
        }
    }

    [UnmanagedCallersOnly]
    private static int SetPrivateDataInterface(StandInPipelineObject* self, Guid* guid, nint data)
    {
        try
        {
            nint desktop = Volatile.Read(ref self->Desktop);

            if (desktop == 0)
            {
                return HResult.Ok;
            }

            return ((delegate* unmanaged<nint, Guid*, nint, int>)ComObject.Slot(desktop, SetPrivateDataInterfaceSlot))(desktop, guid, data);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "StandInPipelineMethods.SetPrivateDataInterface");

            return HResult.Fail;
        }
    }

    [UnmanagedCallersOnly]
    private static int SetName(StandInPipelineObject* self, char* name)
    {
        try
        {
            nint desktop = Volatile.Read(ref self->Desktop);

            if (desktop == 0)
            {
                return HResult.Ok;
            }

            return ((delegate* unmanaged<nint, char*, int>)ComObject.Slot(desktop, SetNameSlot))(desktop, name);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "StandInPipelineMethods.SetName");

            return HResult.Fail;
        }
    }

    /// <summary>The game's device wrapper, as for every wrapped object.</summary>
    [UnmanagedCallersOnly]
    private static int GetDevice(StandInPipelineObject* self, Guid* iid, nint* device)
    {
        try
        {
            nint wrapper = ShimContext.Runtime.D3D12X.Objects.Device;

            if (device == null || wrapper == 0)
            {
                return HResult.Fail;
            }

            return ((delegate* unmanaged<nint, Guid*, nint*, int>)ComObject.Slot(wrapper, ComObject.QueryInterfaceSlot))(wrapper, iid, device);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "StandInPipelineMethods.GetDevice");

            return HResult.Fail;
        }
    }

    [UnmanagedCallersOnly]
    private static int GetCachedBlob(StandInPipelineObject* self, nint* blob)
    {
        try
        {
            nint desktop = Volatile.Read(ref self->Desktop);

            if (desktop == 0)
            {
                return HResult.Fail;
            }

            return ((delegate* unmanaged<nint, nint*, int>)ComObject.Slot(desktop, GetCachedBlobSlot))(desktop, blob);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "StandInPipelineMethods.GetCachedBlob");

            return HResult.Fail;
        }
    }
}
