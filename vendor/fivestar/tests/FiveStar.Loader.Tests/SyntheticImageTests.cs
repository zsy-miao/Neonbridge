// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;
using FiveStar.Loader.Loading;
using FiveStar.Loader.Tests.Support;
using Xunit;

namespace FiveStar.Loader.Tests;

/// <summary>The loader on small images built in memory: mapping, relocation, import binding and TLS.</summary>
public sealed unsafe class SyntheticImageTests
{
    [Fact]
    public void An_image_relocates_and_binds_imports_from_shims_images_and_system32()
    {
        using var test = TestRuntime.Create(["fs_answer.dll"]);
        var answer = new SyntheticImage("fs_answer.dll", TestNative.FreeAddress());
        answer.Code(0, 0xB8, 42, 0, 0, 0, 0xC3);
        answer.Export("Answer", SyntheticImage.TextRva);
        answer.WriteTo(test.Folder);

        // The main image's preferred base is taken, so it must relocate.
        nint occupied = TestNative.Reserve();
        var main = new SyntheticImage("fs_main.dll", (ulong)occupied);

        main.Import("fs_answer.dll", "Answer");
        main.Import("kernel32.dll", "GetCurrentProcessId");
        main.Import("xmem.dll", "XMemAlloc");
        main.Import("xmem.dll", "XMemNoSuchFunction");
        main.WritePointer(0, SyntheticImage.DataRva + 8);
        var (path, slots) = main.WriteTo(test.Folder);

        var image = test.Runtime.Loader.Load(path, true);
        TestNative.Release(occupied);

        var sources = image.Imports.ToDictionary(i => i.Symbol, i => i.Source);
        nint answerSlot = *(nint*)image.Va(slots["fs_answer.dll!Answer"]);
        nint processIdSlot = *(nint*)image.Va(slots["kernel32.dll!GetCurrentProcessId"]);

        Assert.True(image.Relocated);
        Assert.Equal((ulong)image.Va(SyntheticImage.DataRva + 8), *(ulong*)image.Va(SyntheticImage.DataRva));

        Assert.Equal(ResolutionSource.Image, sources["Answer"]);
        Assert.Equal(ResolutionSource.System, sources["GetCurrentProcessId"]);
        Assert.Equal(ResolutionSource.Shim, sources["XMemAlloc"]);
        Assert.Equal(ResolutionSource.Unresolved, sources["XMemNoSuchFunction"]);

        Assert.Equal(42, ((delegate* unmanaged<int>)answerSlot)());
        Assert.Equal((uint)Environment.ProcessId, ((delegate* unmanaged<uint>)processIdSlot)());
        Assert.Equal(test.Shim("xmem.dll", "XMemAlloc"), *(nint*)image.Va(slots["xmem.dll!XMemAlloc"]));
    }

    [Fact]
    public void Implicit_tls_gives_every_thread_its_own_copy_of_the_template()
    {
        using var test = TestRuntime.Create();
        var builder = new SyntheticImage("fs_tls.dll", TestNative.FreeAddress());

        // .data: +0x00 _tls_index, +0x10 the 4-byte template, +0x20 IMAGE_TLS_DIRECTORY64.
        BinaryPrimitives.WriteUInt32LittleEndian(builder.Data.AsSpan(0x10), 0x5A5A1234);
        builder.WritePointer(0x20, SyntheticImage.DataRva + 0x10);
        builder.WritePointer(0x28, SyntheticImage.DataRva + 0x14);
        builder.WritePointer(0x30, SyntheticImage.DataRva);
        builder.SetTlsDirectory(SyntheticImage.DataRva + 0x20);

        // ReadTls: mov eax,[rip+_tls_index]; mov rcx,gs:[58h]; mov rcx,[rcx+rax*8]; mov eax,[rcx]; ret
        builder.Code(0x00, 0x8B, 0x05, 0xFA, 0x0F, 0, 0, 0x65, 0x48, 0x8B, 0x0C, 0x25, 0x58, 0, 0, 0, 0x48, 0x8B, 0x0C, 0xC1, 0x8B, 0x01, 0xC3);

        // WriteTls(ecx): mov eax,[rip+_tls_index]; mov rdx,gs:[58h]; mov rdx,[rdx+rax*8]; mov [rdx],ecx; ret
        builder.Code(0x40, 0x8B, 0x05, 0xBA, 0x0F, 0, 0, 0x65, 0x48, 0x8B, 0x14, 0x25, 0x58, 0, 0, 0, 0x48, 0x8B, 0x14, 0xC2, 0x89, 0x0A, 0xC3);
        builder.Export("ReadTls", SyntheticImage.TextRva);
        builder.Export("WriteTls", SyntheticImage.TextRva + 0x40);
        var (path, _) = builder.WriteTo(test.Folder);

        var image = test.Runtime.Loader.Load(path, true);
        var read = (delegate* unmanaged<uint>)image.Exports.Find("ReadTls", 0)!.Address;
        var write = (delegate* unmanaged<uint, void>)image.Exports.Find("WriteTls", 0)!.Address;

        write(99);
        uint otherThread = 0;
        var thread = new Thread(() => otherThread = ((delegate* unmanaged<uint>)image.Exports.Find("ReadTls", 0)!.Address)());
        thread.Start();
        thread.Join();

        Assert.NotEqual(0u, image.TlsIndex);
        Assert.Equal(99u, read());
        Assert.Equal(0x5A5A1234u, otherThread);
    }
}
