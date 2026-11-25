// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Cli.Progress;

/// <summary>
/// Squarified treemap layout (Bruls, Huizing and van Wijk, 2000): groups, largest first, fill rows along the shorter
/// side while that keeps the worst aspect ratio of the row from growing.
/// </summary>
internal static class Squarifier
{
    public static List<TreemapTile> Layout(IReadOnlyList<ProgressGroup> groups, double x, double y, double width, double height)
    {
        var tiles = new List<TreemapTile>();
        var sorted = groups.Where(group => group.Total > 0).OrderByDescending(group => group.Total).ToList();
        var total = sorted.Sum(group => group.Total);

        if (sorted.Count == 0 || width <= 0 || height <= 0)
        {
            return tiles;
        }

        var scale = width * height / total;
        var areas = sorted.Select(group => group.Total * scale).ToList();
        var next = 0;

        while (next < sorted.Count)
        {
            var side = Math.Min(width, height);
            var end = RowEnd(areas, next, side);
            var rowArea = areas.Skip(next).Take(end - next).Sum();

            if (width >= height)
            {
                var columnWidth = rowArea / height;
                PlaceColumn(tiles, sorted, areas, next, end, x, y, columnWidth);
                x += columnWidth;
                width -= columnWidth;
            }
            else
            {
                var rowHeight = rowArea / width;
                PlaceRow(tiles, sorted, areas, next, end, x, y, rowHeight);
                y += rowHeight;
                height -= rowHeight;
            }

            next = end;
        }

        return tiles;
    }

    /// <summary>The exclusive end of the row that starts at <paramref name="start"/>.</summary>
    private static int RowEnd(List<double> areas, int start, double side)
    {
        var end = start + 1;

        while (end < areas.Count && Worst(areas, start, end + 1, side) <= Worst(areas, start, end, side))
        {
            end++;
        }

        return end;
    }

    private static double Worst(List<double> areas, int start, int end, double side)
    {
        var sum = 0.0;

        for (var index = start; index < end; index++)
        {
            sum += areas[index];
        }

        var worst = 0.0;

        for (var index = start; index < end; index++)
        {
            var area = areas[index];
            var ratio = Math.Max(side * side * area / (sum * sum), sum * sum / (side * side * area));
            worst = Math.Max(worst, ratio);
        }

        return worst;
    }

    private static void PlaceColumn(List<TreemapTile> tiles, List<ProgressGroup> groups, List<double> areas, int start, int end, double x, double y, double columnWidth)
    {
        for (var index = start; index < end; index++)
        {
            var tileHeight = areas[index] / columnWidth;
            tiles.Add(new TreemapTile(x, y, columnWidth, tileHeight, groups[index]));
            y += tileHeight;
        }
    }

    private static void PlaceRow(List<TreemapTile> tiles, List<ProgressGroup> groups, List<double> areas, int start, int end, double x, double y, double rowHeight)
    {
        for (var index = start; index < end; index++)
        {
            var tileWidth = areas[index] / rowHeight;
            tiles.Add(new TreemapTile(x, y, tileWidth, rowHeight, groups[index]));
            x += tileWidth;
        }
    }
}
