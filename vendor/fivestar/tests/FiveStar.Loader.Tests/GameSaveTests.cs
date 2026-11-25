// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Tests.Support;
using Xunit;

namespace FiveStar.Loader.Tests;

/// <summary>XGameSave on a temporary folder, through its native entry points.</summary>
public sealed unsafe class GameSaveTests
{
    [Fact]
    public void Saves_round_trip_through_the_folder_store()
    {
        using var test = TestRuntime.Create();
        NativeCallbacks.Reset();
        nint user = AddUser(test);
        var data = Enumerable.Range(0, 1000).Select(i => (byte)(i * 7)).ToArray();
        nint provider = 0;
        nint container = 0;

        // Provider, container and one update with two blobs.
        fixed (byte* config = "test-config\0"u8)
        {
            ((delegate* unmanaged<nint, byte*, byte, nint*, int>)test.Gdk("XGameSaveInitializeProvider"))(user, config, 0, &provider);
        }

        fixed (byte* name = "slot/1\0"u8)
        {
            ((delegate* unmanaged<nint, byte*, nint*, int>)test.Gdk("XGameSaveCreateContainer"))(provider, name, &container);
        }

        int submitted = WriteBlobs(test, container, data);

        // Enumerate, then read one blob back.
        var enumerate = (delegate* unmanaged<nint, nint, nint, int>)test.Gdk("XGameSaveEnumerateContainerInfo");
        enumerate(provider, 0, (nint)(delegate* unmanaged<nint, nint, byte>)&NativeCallbacks.ContainerInfo);
        var containers = NativeCallbacks.Items.ToList();
        byte[] read = ReadBlob(test, container, "data");

        // Delete the container.
        int deleted;

        fixed (byte* name = "slot/1\0"u8)
        {
            deleted = ((delegate* unmanaged<nint, byte*, int>)test.Gdk("XGameSaveDeleteContainer"))(provider, name);
        }

        Assert.Equal(0, submitted);
        Assert.Equal(["slot/1|Slot One|2|1010"], containers);
        Assert.Equal(data, read);
        Assert.Equal(0, deleted);
        Assert.Empty(Directory.EnumerateDirectories(test.SaveRoot, "slot*", SearchOption.AllDirectories));
    }

    [Fact]
    public void A_read_buffer_sized_for_the_blobs_and_their_data_only_still_reads()
    {
        using var test = TestRuntime.Create();
        nint user = AddUser(test);
        var data = Enumerable.Range(0, 100).Select(i => (byte)i).ToArray();
        nint provider = 0;
        nint container = 0;

        fixed (byte* config = "test-config\0"u8)
        {
            ((delegate* unmanaged<nint, byte*, byte, nint*, int>)test.Gdk("XGameSaveInitializeProvider"))(user, config, 0, &provider);
        }

        fixed (byte* name = "Player\0"u8)
        {
            ((delegate* unmanaged<nint, byte*, nint*, int>)test.Gdk("XGameSaveCreateContainer"))(provider, name, &container);
        }

        WriteBlobs(test, container, data);
        var read = (delegate* unmanaged<nint, byte**, uint*, nuint, byte*, int>)test.Gdk("XGameSaveReadBlobData");
        nuint size = (nuint)((2 * 24) + data.Length + 10);
        byte* buffer = (byte*)NativeMemory.Alloc(size);
        uint count = 0;

        int hr = read(container, null, &count, size, buffer);
        string? second = Marshal.PtrToStringUTF8(*(nint*)(buffer + 24));
        NativeMemory.Free(buffer);

        Assert.Equal(0, hr);
        Assert.Equal(2u, count);
        Assert.Equal("thumb", second);
    }

    [Fact]
    public void An_update_waits_out_another_process_holding_a_blob_instead_of_failing()
    {
        using var test = TestRuntime.Create();
        nint user = AddUser(test);
        nint provider = 0;
        nint container = 0;

        fixed (byte* config = "test-config\0"u8)
        {
            ((delegate* unmanaged<nint, byte*, byte, nint*, int>)test.Gdk("XGameSaveInitializeProvider"))(user, config, 0, &provider);
        }

        fixed (byte* name = "Autosave\0"u8)
        {
            ((delegate* unmanaged<nint, byte*, nint*, int>)test.Gdk("XGameSaveCreateContainer"))(provider, name, &container);
        }

        WriteBlobs(test, container, [1, 2, 3]);
        string blob = Directory.EnumerateFiles(test.SaveRoot, "data", SearchOption.AllDirectories).Single();

        // A scanner opens the new blob without sharing deletion, as Defender may, and lets go 200 ms later.
        var scan = new FileStream(blob, FileMode.Open, FileAccess.Read, FileShare.Read);
        var release = new Thread(() =>
        {
            Thread.Sleep(200);
            scan.Dispose();
        });
        release.Start();
        int hr = WriteBlobs(test, container, [4, 5, 6, 7]);
        release.Join();

        Assert.Equal(0, hr);
        Assert.Equal([4, 5, 6, 7], ReadBlob(test, container, "data"));
    }

    private static nint AddUser(TestRuntime test)
    {
        nint block = TestNative.AsyncBlock(0, 0);
        nint user = 0;
        ((delegate* unmanaged<uint, nint, int>)test.Gdk("XUserAddAsync"))(1, block);
        ((delegate* unmanaged<nint, byte, int>)test.Gdk("XAsyncGetStatus"))(block, 1);
        ((delegate* unmanaged<nint, nint*, int>)test.Gdk("XUserAddResult"))(block, &user);

        return user;
    }

    private static int WriteBlobs(TestRuntime test, nint container, byte[] data)
    {
        var write = (delegate* unmanaged<nint, byte*, byte*, nuint, int>)test.Gdk("XGameSaveSubmitBlobWrite");
        nint update = 0;

        fixed (byte* display = "Slot One\0"u8)
        {
            ((delegate* unmanaged<nint, byte*, nint*, int>)test.Gdk("XGameSaveCreateUpdate"))(container, display, &update);
        }

        fixed (byte* name = "data\0"u8)
        fixed (byte* bytes = data)
        {
            write(update, name, bytes, (nuint)data.Length);
        }

        fixed (byte* name = "thumb\0"u8)
        fixed (byte* bytes = "0123456789"u8)
        {
            write(update, name, bytes, 10);
        }

        return ((delegate* unmanaged<nint, int>)test.Gdk("XGameSaveSubmitUpdate"))(update);
    }

    private static byte[] ReadBlob(TestRuntime test, nint container, string blob)
    {
        var read = (delegate* unmanaged<nint, byte**, uint*, nuint, byte*, int>)test.Gdk("XGameSaveReadBlobData");
        byte* buffer = (byte*)NativeMemory.Alloc(4096);
        uint count = 1;
        nint name = Marshal.StringToHGlobalAnsi(blob);
        byte** names = stackalloc byte*[1];
        names[0] = (byte*)name;

        read(container, names, &count, 4096, buffer);
        uint size = *(uint*)(buffer + 8);
        byte[] result = new ReadOnlySpan<byte>((void*)*(nint*)(buffer + 16), (int)size).ToArray();
        Marshal.FreeHGlobal(name);
        NativeMemory.Free(buffer);

        return result;
    }
}
