// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;
using FiveStar.Shaders.Containers;
using Xunit;

namespace FiveStar.Shaders.Tests;

/// <summary>Bare DXBC containers inside a module (XboxBvhBuilder.dll's builder programs) are found by magic and size.</summary>
public sealed class EmbeddedContainersTests
{
    private const int ContainerAt = 37;
    private const int ChunkCountOffset = 0x1C;

    [Fact]
    public void A_container_is_found_where_its_size_fits_and_its_header_parses()
    {
        var module = new byte[200];

        // A stray magic whose size runs past the file, then a whole empty container.
        "DXBC"u8.CopyTo(module.AsSpan(4));
        BinaryPrimitives.WriteUInt32LittleEndian(module.AsSpan(4 + DxbcContainer.TotalSizeOffset), 0x10000);
        "DXBC"u8.CopyTo(module.AsSpan(ContainerAt));
        BinaryPrimitives.WriteUInt32LittleEndian(module.AsSpan(ContainerAt + DxbcContainer.TotalSizeOffset), DxbcContainer.HeaderSize);
        BinaryPrimitives.WriteInt32LittleEndian(module.AsSpan(ContainerAt + ChunkCountOffset), 0);

        var found = EmbeddedContainers.Find(module);

        var single = Assert.Single(found);
        Assert.Equal(ContainerAt, single.Offset);
        Assert.Equal(DxbcContainer.HeaderSize, single.Container.Length);
    }
}
