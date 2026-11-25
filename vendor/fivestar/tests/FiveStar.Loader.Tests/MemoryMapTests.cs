// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;
using Xunit;

namespace FiveStar.Loader.Tests;

/// <summary>The --memory-map walk: an allocation's committed bytes, and the module its first page points into.</summary>
public sealed unsafe class MemoryMapTests
{
    [Fact]
    public void The_walk_counts_committed_bytes_and_the_hint_names_the_module_a_header_points_into()
    {
        const nuint reserved = 4 << 20;
        const nuint committed = 1 << 20;
        nint allocation = Kernel32.VirtualAlloc(0, reserved, Win32Constants.MemReserve, Win32Constants.PageReadWrite);
        Assert.NotEqual(0, allocation);

        try
        {
            Assert.NotEqual(0, Kernel32.VirtualAlloc(allocation, committed, Win32Constants.MemCommit, Win32Constants.PageReadWrite));

            // An allocator header with a pointer into kernel32, as a vtable or callback would be.
            fixed (char* name = "kernel32.dll")
            {
                *(nint*)allocation = SystemModules.Export(Kernel32.GetModuleHandleW(name), "GetCurrentProcess", 0);
            }

            var walk = MemoryRegionWalk.Run();

            Assert.Equal((long)committed, walk.Allocations[allocation].Private);
            Assert.Equal((long)reserved, walk.Allocations[allocation].Reserved);
            Assert.Contains("kernel", ModulePointers.Loaded().Hint(allocation), StringComparison.OrdinalIgnoreCase);
        }
        finally
        {
            _ = Kernel32.VirtualFree(allocation, 0, Win32Constants.MemRelease);
        }
    }
}
