// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Cli.Progress;

/// <summary>
/// Fills a group's tile with a grid of cells coloured in order done, partial, not yet, in proportion to the group's
/// numbers. A non-zero share always gets at least one cell, and an unfinished group keeps at least one grey cell.
/// </summary>
internal static class CellGrid
{
    public const string DoneClass = "d";
    public const string PartialClass = "p";
    public const string TodoClass = "t";

    private const double CellGap = 1.5;
    private const double MinCellSize = 0.5;
    private const double CellRadius = 1;

    public static void Draw(SvgCanvas canvas, double x, double y, double width, double height, ProgressGroup group, double cellSize)
    {
        var columns = Math.Max(1, (int)Math.Round(width / cellSize));
        var rows = Math.Max(1, (int)Math.Round(height / cellSize));
        var count = columns * rows;
        var (done, partial) = Split(count, group);
        var cellWidth = width / columns;
        var cellHeight = height / rows;

        for (var index = 0; index < count; index++)
        {
            var cssClass = TodoClass;

            if (index < done)
            {
                cssClass = DoneClass;
            }
            else if (index < done + partial)
            {
                cssClass = PartialClass;
            }

            var row = index / columns;
            var column = index % columns;
            var cellX = x + (column * cellWidth);
            var cellY = y + (row * cellHeight);
            canvas.Rect(cssClass, cellX, cellY, Math.Max(MinCellSize, cellWidth - CellGap), Math.Max(MinCellSize, cellHeight - CellGap), CellRadius);
        }
    }

    /// <summary>How many of <paramref name="count"/> cells are done and partial.</summary>
    private static (int Done, int Partial) Split(int count, ProgressGroup group)
    {
        var total = group.Total;

        if (total <= 0)
        {
            return (0, 0);
        }

        var done = (int)Math.Round(count * group.Done / total);
        var partial = (int)Math.Round(count * group.Partial / total);

        if (group.Done > 0 && done == 0)
        {
            done = 1;
        }

        if (group.Partial > 0 && partial == 0)
        {
            partial = 1;
        }

        while (group.Todo > 0 && done + partial >= count && done + partial > 0)
        {
            if (done >= partial)
            {
                done--;
            }
            else
            {
                partial--;
            }
        }

        return (done, partial);
    }
}
