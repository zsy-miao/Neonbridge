// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;
using FiveStar.Loader.Shims.D3D12X.DebugLayer;
using Xunit;

namespace FiveStar.Loader.Tests;

/// <summary>The depth-UAV instrumentation: heap-index records and the depth state line of a lighting pipeline.</summary>
public sealed unsafe class DepthUavsTests
{
    [Fact]
    public void A_record_names_a_heap_index_only_without_console_fields_and_depth_state_reads_as_text()
    {
        uint* index = stackalloc uint[] { 92380, 0, 0, 0, 92380, 0, 0, 0 };
        uint* texture = stackalloc uint[] { 0x00168dc0, 0x000002b9, 0x01ff00ff, 0x90000000, 0, 0, 0, 0 };

        Assert.Equal(92380u, DepthUavs.HeapIndex(index));
        Assert.Null(DepthUavs.HeapIndex(texture));

        // Depth on, no writes, EQUAL (3); stencil on, read 0xff, write 0x00, both faces KEEP KEEP KEEP EQUAL.
        var desc = new byte[0x40];
        int[] words = [1, 0, 3, 1];

        for (int i = 0; i < words.Length; i++)
        {
            BinaryPrimitives.WriteInt32LittleEndian(desc.AsSpan(i * sizeof(int)), words[i]);
        }

        desc[0x10] = 0xff;

        foreach (int face in (int[])[0x14, 0x24])
        {
            int[] ops = [1, 1, 1, 3];

            for (int i = 0; i < ops.Length; i++)
            {
                BinaryPrimitives.WriteInt32LittleEndian(desc.AsSpan(face + (i * sizeof(int))), ops[i]);
            }
        }

        Assert.Equal("depth 1 write 0 func 3; stencil 1 read 0xff write 0x00 front (fail depth-fail pass func) 1 1 1 3 back 1 1 1 3", DepthUavs.DescribeDepthState(desc));
    }
}
