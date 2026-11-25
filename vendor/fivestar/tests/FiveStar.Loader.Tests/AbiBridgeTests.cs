// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Tests.Support;
using Xunit;

namespace FiveStar.Loader.Tests;

/// <summary>The XGameRuntime thunk ABI: QueryApiImpl objects whose vtable slots reach the named shims.</summary>
public sealed unsafe class AbiBridgeTests
{
    [Fact]
    public void QueryApiImpl_routes_vtable_slots_through_drop_this_thunks()
    {
        using var test = TestRuntime.Create();
        NativeCallbacks.Reset();
        var clsid = new Guid("11111111-2222-3333-4444-555555555555");
        var iid = new Guid("66666666-7777-8888-9999-aaaaaaaaaaaa");
        var other = Guid.NewGuid();
        var abi = test.Runtime.Gdk.Runtime;

        // Slot 7 stays unmapped.
        abi.RegisterAbi(clsid, iid, 3, "XTaskQueueCreate");
        abi.RegisterAbi(clsid, iid, 4, "XTaskQueueSubmitCallback");
        abi.RegisterAbi(clsid, iid, 5, "XTaskQueueDispatch");
        abi.RegisterAbi(clsid, iid, 6, "XTaskQueueRegisterWaiter");
        abi.RegisterAbi(clsid, iid, 8, "XTaskQueueCloseHandle");

        var query = (delegate* unmanaged<Guid*, Guid*, nint*, int>)test.Gdk("QueryApiImpl");
        nint instance = 0;
        nint none = 0;
        int hr = query(&clsid, &iid, &instance);
        int unknown = query(&other, &iid, &none);
        nint* vtable = *(nint**)instance;

        // Create a queue, submit a callback, register a waiter (7 arguments with this: two on the stack), signal, dispatch.
        delegate* unmanaged<nint, byte, void> record = &NativeCallbacks.Record;
        nint queue = 0;
        ulong token = 0;
        nint signal = TestNative.CreateEvent(0, 0, 0, 0);
        ((delegate* unmanaged<nint, uint, uint, nint*, int>)vtable[3])(instance, 0, 0, &queue);
        ((delegate* unmanaged<nint, nint, uint, nint, nint, int>)vtable[4])(instance, queue, 0, 0x51, (nint)record);

        byte first = ((delegate* unmanaged<nint, nint, uint, uint, byte>)vtable[5])(instance, queue, 0, 0);
        int waiter = ((delegate* unmanaged<nint, nint, uint, nint, nint, nint, ulong*, int>)vtable[6])(instance, queue, 0, signal, 0x52, (nint)record, &token);
        _ = TestNative.SetEvent(signal);
        byte second = ((delegate* unmanaged<nint, nint, uint, uint, byte>)vtable[5])(instance, queue, 0, 2000);
        int unmapped = ((delegate* unmanaged<nint, int>)vtable[7])(instance);
        ((delegate* unmanaged<nint, nint, void>)vtable[8])(instance, queue);

        _ = TestNative.CloseHandle(signal);

        Assert.Equal(0, hr);
        Assert.Equal(unchecked((int)0x80040154), unknown);
        Assert.Equal(1, first);
        Assert.Equal(0, waiter);
        Assert.Equal(1, second);
        Assert.Equal([0x51, 0x52], NativeCallbacks.Drain().Select(c => c.Context));

        Assert.Equal(unchecked((int)0x80004001), unmapped);
    }
}
