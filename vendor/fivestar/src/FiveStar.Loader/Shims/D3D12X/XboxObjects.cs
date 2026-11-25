// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Codegen;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Loading;
using FiveStar.Loader.Native;
using FiveStar.Loader.Shims.D3D12X.DebugLayer;

namespace FiveStar.Loader.Shims.D3D12X;

/// <summary>
/// The wrappers the D3D12.X shim hands the game: one vtable per <see cref="XboxInterfaceKind"/>, all in one native
/// block so a pointer is recognised as a wrapper by where its vtable lies. Slots forward to the desktop object through
/// generated thunks (<see cref="XboxInterfaces"/>), X-only and unknown slots are stubs that log their first call with
/// the caller, and <see cref="SetHandler"/> replaces any slot with a native function (other lanes' entry point).
/// </summary>
internal sealed unsafe class XboxObjects
{
    private const int QueryInterfaceSlot = 0;
    private const int ReleaseSlot = 2;

    /// <summary>ID3D12DeviceChild::GetDevice, the last base slot of every interface except the device.</summary>
    private const int GetDeviceSlot = 7;

    /// <summary>ID3D12Device::GetDeviceRemovedReason in the desktop vtable (d3d12.h).</summary>
    private const int GetDeviceRemovedReasonSlot = 37;

    private readonly object _gate = new();
    private readonly Dictionary<(XboxInterfaceKind Kind, int Slot), nint> _handlers = [];
    private readonly HashSet<(XboxInterfaceKind Kind, int Slot)> _skipped = [];
    private readonly UnresolvedStubs _stubs;
    private readonly CodeHeap _heap;
    private readonly D3D12Debugging _debugging;
    private readonly LoaderLog _log;
    private readonly nint* _vtables;
    private readonly int _vtableBytes;
    private bool _built;
    private nint _device;
    private nint _desktopDevice;

    public XboxObjects(UnresolvedStubs stubs, CodeHeap heap, D3D12Debugging debugging, LoaderLog log)
    {
        _stubs = stubs;
        _heap = heap;
        _debugging = debugging;
        _log = log;
        _vtableBytes = XboxInterfaces.Count * XboxInterfaces.MaxSlotCount * sizeof(nint);
        _vtables = (nint*)NativeMemory.AllocZeroed((nuint)_vtableBytes);
    }

    /// <summary>The device wrapper the game holds (GetDevice on any child returns it); 0 before device creation.</summary>
    public nint Device => Volatile.Read(ref _device);

    /// <summary>The desktop device behind <see cref="Device"/>; 0 before device creation.</summary>
    public nint DesktopDevice => Volatile.Read(ref _desktopDevice);

    /// <summary>
    /// Binds <paramref name="slot"/> of <paramref name="kind"/>'s vtable to <paramref name="handler"/>, a native
    /// function taking the wrapper as its first argument (an <c>[UnmanagedCallersOnly]</c> method or generated code).
    /// Works before and after the vtables exist; the last registration for a slot wins.
    /// </summary>
    public void SetHandler(XboxInterfaceKind kind, int slot, nint handler)
    {
        ArgumentOutOfRangeException.ThrowIfNegative(slot);
        ArgumentOutOfRangeException.ThrowIfGreaterThanOrEqual(slot, XboxInterfaces.For(kind).SlotCount);

        lock (_gate)
        {
            if (_skipped.Contains((kind, slot)))
            {
                return;
            }

            Install(kind, slot, handler);
        }
    }

    /// <summary>
    /// <c>--d3d12-skip</c>: the slot does nothing and returns 0 from now on, whatever handler is bound to it before
    /// or after (a bisection aid for GPU faults).
    /// </summary>
    public void Skip(SkippedSlot skipped)
    {
        ArgumentOutOfRangeException.ThrowIfGreaterThanOrEqual(skipped.Slot, XboxInterfaces.For(skipped.Kind).SlotCount);
        delegate* unmanaged<XboxObject*, nint> nothing = &XboxObjectMethods.Skipped;

        lock (_gate)
        {
            _skipped.Add((skipped.Kind, skipped.Slot));
            Install(skipped.Kind, skipped.Slot, (nint)nothing);
        }

        _log.Warn($"[d3d12_xs] --d3d12-skip: {XboxInterfaces.For(skipped.Kind).Name} slot {skipped.Slot} does nothing");
    }

    private void Install(XboxInterfaceKind kind, int slot, nint handler)
    {
        _handlers[(kind, slot)] = handler;

        if (_built)
        {
            Volatile.Write(ref VtableOf(kind)[slot], handler);
        }
    }

    /// <summary>Wraps a desktop object (0 stays 0). The wrapper is zeroed past its header.</summary>
    public nint Wrap(XboxInterfaceKind kind, nint inner)
    {
        if (inner == 0)
        {
            return 0;
        }

        EnsureBuilt();
        var wrapper = (XboxObject*)NativeMemory.AllocZeroed((nuint)XboxInterfaces.For(kind).ObjectSize);
        wrapper->Vtable = (nint)VtableOf(kind);
        wrapper->Inner = inner;

        return (nint)wrapper;
    }

    /// <summary>Wraps a resource that lives at a console GPU address the game chose.</summary>
    public nint WrapResource(nint inner, ulong gameAddress)
    {
        nint wrapper = Wrap(XboxInterfaceKind.Resource, inner);

        if (wrapper != 0)
        {
            ((XboxResourceObject*)wrapper)->GameAddress = gameAddress;
        }

        return wrapper;
    }

    /// <summary>Wraps the device and remembers it for GetDevice.</summary>
    public nint WrapDevice(nint inner)
    {
        nint wrapper = Wrap(XboxInterfaceKind.Device, inner);

        if (wrapper != 0)
        {
            ((XboxDeviceObject*)wrapper)->DescriptorCalls = XboxDeviceObject.ThroughSlots;
        }

        Volatile.Write(ref _desktopDevice, inner);
        Volatile.Write(ref _device, wrapper);

        return wrapper;
    }

    /// <summary>Replaces <c>*result</c> by its wrapper when <paramref name="hr"/> succeeded; returns <paramref name="hr"/>.</summary>
    public int WrapResult(int hr, XboxInterfaceKind kind, nint* result)
    {
        if (HResult.Failed(hr))
        {
            _log.Warn($"[d3d12_xs] {XboxInterfaces.For(kind).Name} creation failed: {HResult.Format(hr)} (device removed reason {HResult.Format(RemovedReason())})");
            _debugging.OnFailure();

            return hr;
        }

        if (result == null)
        {
            return hr;
        }

        *result = Wrap(kind, *result);

        return hr;
    }

    /// <summary>The desktop object behind a wrapper; any other pointer (0, a desktop object) is returned as is.</summary>
    [HotPath]
    public nint Unwrap(nint pointer)
    {
        if (pointer == 0)
        {
            return 0;
        }

        nint vtable = *(nint*)pointer;

        if ((nuint)(vtable - (nint)_vtables) >= (nuint)_vtableBytes)
        {
            return pointer;
        }

        return ((XboxObject*)pointer)->Inner;
    }

    /// <summary>The resource wrapper <paramref name="pointer"/> is, or null for any other pointer.</summary>
    [HotPath]
    public XboxResourceObject* AsResource(nint pointer)
    {
        if (pointer == 0 || *(nint*)pointer != (nint)VtableOf(XboxInterfaceKind.Resource))
        {
            return null;
        }

        return (XboxResourceObject*)pointer;
    }

    /// <summary>The device wrapper when <paramref name="desktop"/> is the desktop device behind it, else <paramref name="desktop"/>.</summary>
    [HotPath]
    public nint DeviceFor(nint desktop)
    {
        if (desktop != 0 && desktop == Volatile.Read(ref _desktopDevice))
        {
            return Volatile.Read(ref _device);
        }

        return desktop;
    }

    /// <summary>ID3D12Device::GetDeviceRemovedReason (desktop slot 37), S_OK while the device works.</summary>
    private int RemovedReason()
    {
        nint device = Volatile.Read(ref _desktopDevice);

        if (device == 0)
        {
            return HResult.Ok;
        }

        return ((delegate* unmanaged<nint, int>)ComObject.Slot(device, GetDeviceRemovedReasonSlot))(device);
    }

    private nint* VtableOf(XboxInterfaceKind kind)
    {
        return _vtables + ((int)kind * XboxInterfaces.MaxSlotCount);
    }

    private void EnsureBuilt()
    {
        lock (_gate)
        {
            if (_built)
            {
                return;
            }

            for (int kind = 0; kind < XboxInterfaces.Count; kind++)
            {
                Build(XboxInterfaces.For((XboxInterfaceKind)kind));
            }

            foreach (var ((kind, slot), handler) in _handlers)
            {
                VtableOf(kind)[slot] = handler;
            }

            // One read-write to read-execute flip for every thunk and stub.
            _heap.Seal();
            _built = true;
        }
    }

    private void Build(XboxInterface type)
    {
        nint* vtable = VtableOf(type.Kind);

        for (int slot = 0; slot < type.SlotCount; slot++)
        {
            vtable[slot] = _stubs.Create(D3D12XLayout.Module, $"{type.Name}.X::Slot{slot}", HResult.NotImplemented, true, false);
        }

        // Command lists count their forwarded calls: a memory wait after any command splits the list (ListSplits).
        int counter = 0;

        if (type.Kind == XboxInterfaceKind.CommandList)
        {
            counter = ListSegments.CommandsOffset;
        }

        foreach (var forwarded in type.Forwards)
        {
            vtable[forwarded.Slot] = ForwardingThunk.Emit(_heap, forwarded, (nint)_vtables, _vtableBytes, counter);
        }

        delegate* unmanaged<XboxObject*, Guid*, nint*, int> queryInterface = &XboxObjectMethods.QueryInterface;
        delegate* unmanaged<XboxObject*, uint> release = &XboxObjectMethods.Release;
        vtable[QueryInterfaceSlot] = (nint)queryInterface;
        vtable[ReleaseSlot] = (nint)release;

        if (type.Kind != XboxInterfaceKind.Device)
        {
            delegate* unmanaged<XboxObject*, Guid*, nint*, int> getDevice = &XboxObjectMethods.GetDevice;
            vtable[GetDeviceSlot] = (nint)getDevice;
        }

        _log.Detail($"  [d3d12_xs] {type.Name} vtable: {type.Forwards.Count} forwarded slots of {type.SlotCount}");
    }
}
