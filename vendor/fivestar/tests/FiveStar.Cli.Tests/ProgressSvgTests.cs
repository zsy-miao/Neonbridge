// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Xml.Linq;
using FiveStar.Cli.Progress;
using Xunit;

namespace FiveStar.Cli.Tests;

/// <summary>The progress report renders as well-formed SVG with one hover area per group.</summary>
public sealed class ProgressSvgTests
{
    private static readonly XNamespace Svg = "http://www.w3.org/2000/svg";

    [Fact]
    public void A_small_status_renders_one_group_area_per_group()
    {
        var status = new ProgressStatus(
            [
                new ProgressArea("shaders", "Shaders", null, 100, [
                    new ProgressGroup("Pixel", 90, 0, 10, "not yet: dxc-error (10)"),
                    new ProgressGroup("Mesh", 5, 3, 0, null),
                ]),
                new ProgressArea("archives", "Archive bytes & <more>", " GiB", null, [
                    new ProgressGroup("a.rpf", 1.25, 0, 0.75, null),
                ]),
            ],
            [new NeededColumn("Engineering", [new NeededItem("First frame", "swap chain and a real draw")])]);

        var document = XDocument.Parse(ProgressSvg.Render(status));

        var hits = document.Descendants(Svg + "rect").Where(rect => (string?)rect.Attribute("class") == "hit").ToList();
        Assert.Equal(3, hits.Count);
        Assert.All(hits, hit => Assert.NotNull(hit.Element(Svg + "title")));
    }
}
