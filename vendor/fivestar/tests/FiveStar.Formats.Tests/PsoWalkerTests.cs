// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Formats.Common;
using FiveStar.Formats.Pso;
using FiveStar.Formats.Tests.Support;
using Xunit;

namespace FiveStar.Formats.Tests;

/// <summary>Parsing and walking a hand-built L-format PSO file.</summary>
public sealed class PsoWalkerTests
{
    private const uint Root = 0xAABBCCDD;

    [Fact]
    public void A_pso_file_parses_and_walks_through_its_array_pointer()
    {
        var document = PsoReader.Load(File());

        var tree = new PsoWalker(document).WalkRoot(PsoWalker.UnlimitedBudget);
        var json = JsonText.Write(tree, 0).Replace("\n", string.Empty, StringComparison.Ordinal);

        Assert.Equal(PsoLayout.File, document.Layout);
        Assert.Equal(2, document.Blocks.Count);
        Assert.Equal(4, document.Structs[Root].Members.Count);
        Assert.Equal("{\"_type\": \"aabbccdd\",\"11111111\": 7,\"22222222\": \"h:deadbeef\",\"33333333\": [1,2,3]}", json);
        Assert.Equal(0xCA2E9442u, Joaat.Hash("A"));
    }

    // PSIN: block table (root struct 0x20 bytes, u16 block 6 bytes), blocks at +0x30 and +0x50. LSCH: one struct with
    // u32, hash, array {count, capacity, pointer to block 2} and its u16 element descriptor. LRSC: PRD0, root block 1.
    private static byte[] File()
    {
        var file = new ByteWriter();
        file.Bytes("PSIN"u8).U32BigEndian(0x60);
        file.U32(Root).U32(0x20).U64(0xAAAAAAAAAAAAAAAA);
        file.U32(0x13).U32(6).U64(0xAAAAAAAAAAAAAAAA);
        file.Align(16, 0x70);

        // Root object: u32 7, hash 0xDEADBEEF, array of 3 at +8 whose pointer (block 2, offset 0) sits at +16.
        file.U32(7).U32(0xDEADBEEF).U32(3).U32(3).U64(2).U64(0);
        file.U16(1).U16(2).U16(3).Align(16, 0x70);

        file.Bytes("LSCH"u8).U32BigEndian(8 + 0x20 + (4 * 16));
        file.U32(Root).U32(0).U32(0x300).U32(0).U64(0xAAAAAAAAAAAAAAAA).U32(0x20).U16(0).U16(4);
        file.U32(0x11111111).U32(0).U32(0x15).U32(0);
        file.U32(0x22222222).U32(4).U32(0x4A).U32(0);
        file.U32(0x33333333).U32(8).U32(0x5C | (3 << 16)).U32(0);
        file.U32(0x100).U32(0).U32(0x13).U32(0);

        file.Bytes("LRSC"u8).U32BigEndian(8 + 0x18).Bytes("0DRP"u8).U32(0x7A).U64(0).U32(1).U32(0);
        var checksum = file.Position;
        file.Bytes("CHKS"u8).U32BigEndian(20).U32BigEndian(0).U32(0).Bytes("zppp"u8);
        file.PatchU32BigEndian(checksum + 8, (uint)file.Position);

        return file.ToArray();
    }
}
