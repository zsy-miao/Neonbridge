// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Tests.Support;
using Xunit;

namespace FiveStar.Loader.Tests;

/// <summary>Module identity and dynamic loading as mapped code sees them, through the entry points its IAT would hold.</summary>
public sealed unsafe class ModuleIdentityTests
{
    [Fact]
    public void Module_shims_answer_for_the_main_image_and_for_shim_only_modules()
    {
        using var test = TestRuntime.Create();
        var builder = new SyntheticImage("fs_identity.dll", TestNative.FreeAddress());
        var (path, _) = builder.WriteTo(test.Folder);
        var main = test.Runtime.Loader.Load(path, true);

        var getModuleHandle = (delegate* unmanaged<char*, nint>)test.Shim("kernel32.dll", "GetModuleHandleW");
        var getModuleFileName = (delegate* unmanaged<nint, char*, uint, uint>)test.Shim("kernel32.dll", "GetModuleFileNameW");
        var loadLibrary = (delegate* unmanaged<char*, nint>)test.Shim("kernel32.dll", "LoadLibraryW");
        var getProcAddress = (delegate* unmanaged<nint, byte*, nint>)test.Shim("kernel32.dll", "GetProcAddress");

        char* buffer = stackalloc char[16];
        Marshal.SetLastSystemError(0);
        uint truncated = getModuleFileName(0, buffer, 10);
        int error = Marshal.GetLastSystemError();
        nint xmem;

        fixed (char* name = "xmem.dll")
        {
            xmem = loadLibrary(name);
        }

        nint alloc;

        fixed (byte* name = "XMemAlloc\0"u8)
        {
            alloc = getProcAddress(xmem, name);
        }

        Assert.Equal(main.Base, getModuleHandle(null));
        Assert.Equal(10u, truncated);
        Assert.Equal(main.Path[..9], new string(buffer, 0, 9));
        Assert.Equal(122, error);
        Assert.Equal("xmem.dll", test.Runtime.VirtualModules.NameOf(xmem));
        Assert.Equal(test.Shim("xmem.dll", "XMemAlloc"), alloc);
    }
}
