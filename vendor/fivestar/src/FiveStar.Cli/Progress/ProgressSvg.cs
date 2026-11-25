// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Text;

namespace FiveStar.Cli.Progress;

/// <summary>
/// Renders a <see cref="ProgressStatus"/> as one self-contained SVG: per area a headline and a squarified treemap of
/// its groups, then the still-needed list, a legend and a footer. Light and dark themes follow
/// <c>prefers-color-scheme</c>.
/// </summary>
public static class ProgressSvg
{
    private const double Width = 900;
    private const double Padding = 16;
    private const double LegendSpace = 4;
    private const double LegendSwatch = 12;
    private const double LegendSwatchRadius = 2;
    private const double LegendTextOffset = 18;
    private const double LegendBaseline = 10.5;
    private const double LegendSize = 12;
    private const double LegendCharWidth = 6.6;
    private const double LegendItemSpace = 30;
    private const double LegendHeight = 22;
    private const double FooterSize = 11;
    private const double FooterFirstLine = 10;
    private const double FooterLine = 15;
    private const double BottomSpace = 11;

    private const string Css = """
        svg { font-family: system-ui, -apple-system, "Segoe UI", sans-serif; }
        .bg { fill: #fcfcfb; } .ink { fill: #0b0b0b; } .ink2 { fill: #52514e; } .mute { fill: #898781; }
        .d { fill: #0ca30c; } .p { fill: #fab219; } .t { fill: #c3c2b7; }
        .pill { fill: #fcfcfb; fill-opacity: .9; } .hit { fill: transparent; } .hit:hover { fill: #0b0b0b; fill-opacity: .08; }
        @media (prefers-color-scheme: dark) {
          .bg { fill: #1a1a19; } .ink { fill: #f0efec; } .ink2 { fill: #c3c2b7; } .mute { fill: #9a9893; }
          .t { fill: #4a4945; } .pill { fill: #1a1a19; } .hit:hover { fill: #ffffff; }
        }
        """;

    private static readonly (string CssClass, string Text)[] LegendItems =
    [
        (CellGrid.DoneClass, "done"),
        (CellGrid.PartialClass, "done through a fallback or stand-in"),
        (CellGrid.TodoClass, "not yet"),
    ];

    private static readonly string[] FooterLines =
    [
        "Shaders count as compiled when dxc accepts the translated HLSL; the WARP tests check that they compute the right values.",
        "One cell stands for several items in large groups; hover a group for its exact numbers.",
        "Regenerate: fivestar progress collect --shaders <results.json> --dump <dump> --manual <manual.json> status.json,",
        "then fivestar progress status.json progress.svg.",
    ];

    public static string Render(ProgressStatus status)
    {
        var canvas = new SvgCanvas();
        var y = Padding;
        var areas = new AreaSection(canvas, Padding, Width - (Padding + Padding));

        foreach (var area in status.Areas)
        {
            y = areas.Draw(area, y);
        }

        if (status.StillNeeded is not null && status.StillNeeded.Count > 0)
        {
            y = new NeededSection(canvas, Padding, Width - (Padding + Padding)).Draw(status.StillNeeded, y);
        }

        y = DrawLegend(canvas, y + LegendSpace);
        y = DrawFooter(canvas, y);

        return Document(canvas.Body, Math.Ceiling(y + BottomSpace));
    }

    private static double DrawLegend(SvgCanvas canvas, double y)
    {
        var x = Padding;

        foreach (var (cssClass, text) in LegendItems)
        {
            canvas.Rect(cssClass, x, y, LegendSwatch, LegendSwatch, LegendSwatchRadius);
            canvas.Text("ink", x + LegendTextOffset, y + LegendBaseline, LegendSize, text);
            x += LegendItemSpace + (LegendCharWidth * text.Length);
        }

        return y + LegendHeight;
    }

    private static double DrawFooter(SvgCanvas canvas, double y)
    {
        y += FooterFirstLine;

        foreach (var line in FooterLines)
        {
            canvas.Text("mute", Padding, y, FooterSize, line);
            y += FooterLine;
        }

        return y;
    }

    private static string Document(string body, double height)
    {
        var size = $"width=\"{Width}\" height=\"{height}\" viewBox=\"0 0 {Width} {height}\"";
        var svg = new StringBuilder();
        svg.Append("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");

        svg.Append("<svg xmlns=\"http://www.w3.org/2000/svg\" ").Append(size).Append(" role=\"img\" aria-label=\"Five Star Recomp progress\">\n");
        svg.Append("<title>Five Star Recomp progress</title>\n");
        svg.Append("<style>\n").Append(Css).Append("\n</style>\n");
        svg.Append("<rect class=\"bg\" width=\"").Append(Width).Append("\" height=\"").Append(height).Append("\"/>\n");

        svg.Append(body);
        svg.Append("</svg>\n");

        return svg.ToString();
    }
}
