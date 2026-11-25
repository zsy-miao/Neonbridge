// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Codegen;
using FiveStar.Loader.Diagnostics;
using Xunit;

namespace FiveStar.Loader.Tests;

/// <summary>
/// The fault reporter's native entry enters its [UnmanagedCallersOnly] handler only for faults outside managed code:
/// one entered from a managed fault ends the process ("attempted to call a UnmanagedCallersOnly method from managed code").
/// </summary>
public sealed unsafe class FaultEntryTests
{
    /// <summary>A stand-in handler: <c>mov eax,7; ret</c>.</summary>
    private static readonly byte[] Handler = [0xB8, 0x07, 0x00, 0x00, 0x00, 0xC3];
    private const int Entered = 7;
    private const int PassedOn = 0;

    [Fact]
    public void Faults_in_mapped_or_native_code_reach_the_handler_and_managed_ones_do_not()
    {
        var heap = new CodeHeap();
        nint handler = heap.Write(Handler);
        nint mapped = (nint)NativeMemory.AllocZeroed(0x1000);
        ulong* ranges = stackalloc ulong[] { 1, (ulong)mapped, (ulong)mapped + 0x1000 };
        nint pcToModule = NativeLibrary.GetExport(NativeLibrary.Load("ntdll.dll"), "RtlPcToFileHeader");
        NativeLibrary.TryLoad("coreclr.dll", out nint coreclr);

        nint entry = heap.Write(FaultEntryCode.Body(handler, pcToModule, coreclr, 0, (nint)ranges));
        heap.Seal();
        delegate* unmanaged<nint, int> enter = (delegate* unmanaged<nint, int>)entry;

        try
        {
            nint native = NativeLibrary.GetExport(NativeLibrary.Load("kernel32.dll"), "GetTickCount");
            nint managedImage = Marshal.GetHINSTANCE(typeof(object).Module) + 0x1000;
            delegate* managed<int> jitted = &Jitted;

            Assert.Equal(Entered, Fault(enter, mapped + 0x10));
            Assert.Equal(Entered, Fault(enter, native));
            Assert.Equal(PassedOn, Fault(enter, coreclr + 0x1000));
            Assert.Equal(PassedOn, Fault(enter, managedImage));
            Assert.Equal(PassedOn, Fault(enter, (nint)jitted));
        }
        finally
        {
            NativeMemory.Free((void*)mapped);
        }
    }

    private static int Jitted()
    {
        return Environment.ProcessorCount;
    }

    /// <summary>Calls the entry with EXCEPTION_POINTERS whose record names <paramref name="address"/>.</summary>
    private static int Fault(delegate* unmanaged<nint, int> enter, nint address)
    {
        ulong* record = stackalloc ulong[] { 0xC0000005, 0, (ulong)address, 0 };
        nint* pointers = stackalloc nint[] { (nint)record, 0 };

        return enter((nint)pointers);
    }
}
