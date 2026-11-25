// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Shims.D3D12X;
using FiveStar.Loader.Shims.D3D12X.DebugLayer;
using Xunit;

namespace FiveStar.Loader.Tests;

/// <summary>The fix for the GPU hang after the first frame and the bisection option that found it.</summary>
public sealed class D3D12XHangTests
{
    [Theory]
    [InlineData(17u, 4u)]
    [InlineData(4u, 4u)]
    [InlineData(5u, 5u)]
    [InlineData(36u, 36u)]
    public void The_rectangle_list_becomes_a_triangle_list_and_desktop_topologies_pass(uint console, uint desktop)
    {
        Assert.Equal(desktop, PrimitiveTopologies.Desktop(console));
        Assert.True(PrimitiveTopologies.IsDesktop(PrimitiveTopologies.Desktop(console)));
    }

    [Theory]
    [InlineData(0u)]
    [InlineData(6u)]
    [InlineData(18u)]
    [InlineData(65u)]
    public void Topologies_the_desktop_does_not_define_are_not_set(uint console)
    {
        Assert.False(PrimitiveTopologies.IsDesktop(PrimitiveTopologies.Desktop(console)));
    }

    [Fact]
    public void Skip_lists_name_interfaces_by_alias_or_kind()
    {
        var slots = SkippedSlot.ParseList("list:20, queue:35,Device:127");

        Assert.Equal(
            [new SkippedSlot(XboxInterfaceKind.CommandList, 20), new SkippedSlot(XboxInterfaceKind.CommandQueue, 35), new SkippedSlot(XboxInterfaceKind.Device, 127)],
            slots);
        Assert.Throws<FormatException>(() => SkippedSlot.Parse("list"));
    }
}
