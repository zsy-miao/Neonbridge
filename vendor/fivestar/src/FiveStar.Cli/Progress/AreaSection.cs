// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Cli.Progress;

/// <summary>Draws one area: its headline and a treemap of its groups with a label pill and a hover title per group.</summary>
internal sealed class AreaSection
{
    private const double DefaultHeight = 120;
    private const double HeadlineSize = 15;
    private const double HeadlineBaseline = 15;
    private const double HeadlineSpace = 24;
    private const double SpaceAfter = 18;
    private const double GroupGap = 2;
    private const double HalfGap = GroupGap / 2;
    private const double LargeCell = 11;
    private const double MinSmallCell = 6;
    private const double SmallCellDivisor = 3;
    private const double LargeGroup = 40;

    // Label pill: 11 px text in a 16 px pill, inset from the tile corner.
    private const double LabelSize = 11;
    private const double PillHeight = 16;
    private const double PillInset = 2;
    private const double BothInsets = 2 * PillInset;
    private const double PillRadius = 3;
    private const double TextInset = 6;
    private const double TextBaseline = 14;
    private const double PillPadding = 8;
    private const double CharWidth = 6.3;
    private const double MinLabelWidth = 40;
    private const int MinLabelChars = 6;
    private const string Ellipsis = "\u2026";

    private readonly SvgCanvas _canvas;
    private readonly double _left;
    private readonly double _width;

    public AreaSection(SvgCanvas canvas, double left, double width)
    {
        _canvas = canvas;
        _left = left;
        _width = width;
    }

    /// <summary>Draws the area with its top at <paramref name="y"/> and returns the top of what follows.</summary>
    public double Draw(ProgressArea area, double y)
    {
        _canvas.BoldText("ink", _left, y + HeadlineBaseline, HeadlineSize, Headline(area));
        y += HeadlineSpace;
        var height = area.Height ?? DefaultHeight;
        var unit = area.Unit ?? string.Empty;

        foreach (var tile in Squarifier.Layout(area.Groups, _left, y, _width, height))
        {
            DrawGroup(tile, unit);
        }

        return y + height + SpaceAfter;
    }

    private static string Headline(ProgressArea area)
    {
        var total = area.Groups.Sum(group => group.Total);
        var partial = area.Groups.Sum(group => group.Partial);
        var finished = area.Groups.Sum(group => group.Done) + partial;
        var unit = area.Unit ?? string.Empty;
        var text = $"{area.Title}: {ProgressText.Headline(finished, total)}% ({ProgressText.Amount(finished)}/{ProgressText.Amount(total)}{unit})";

        if (partial > 0)
        {
            text += $", {ProgressText.Amount(partial)}{unit} through a fallback or stand-in";
        }

        return text;
    }

    private void DrawGroup(TreemapTile tile, string unit)
    {
        var x = tile.X + HalfGap;
        var y = tile.Y + HalfGap;
        var width = tile.Width - GroupGap;
        var height = tile.Height - GroupGap;
        var group = tile.Group;
        var cellSize = LargeCell;

        if (group.Total <= LargeGroup && unit.Length == 0)
        {
            cellSize = Math.Max(MinSmallCell, Math.Min(width, height) / SmallCellDivisor);
        }

        CellGrid.Draw(_canvas, x, y, width, height, group, cellSize);
        DrawLabel(x, y, width, height, group);
        _canvas.HitArea(x, y, width, height, HoverTitle(group, unit));
    }

    private void DrawLabel(double x, double y, double width, double height, ProgressGroup group)
    {
        if (height < PillHeight + BothInsets || width < MinLabelWidth)
        {
            return;
        }

        var label = FitLabel(group, width - BothInsets - PillPadding);

        if (label is null)
        {
            return;
        }

        var pillWidth = Math.Min(width - BothInsets, PillPadding + (CharWidth * label.Length));
        _canvas.Rect("pill", x + PillInset, y + PillInset, pillWidth, PillHeight, PillRadius);
        _canvas.Text("ink", x + TextInset, y + TextBaseline, LabelSize, label);
    }

    /// <summary>The longest of "name pct%", "name" and a shortened name that fits, or null when none does.</summary>
    private static string? FitLabel(ProgressGroup group, double room)
    {
        var fits = (int)(room / CharWidth);
        var full = $"{group.Name} {ProgressText.Tile(group.Done + group.Partial, group.Total)}%";

        if (full.Length <= fits)
        {
            return full;
        }

        if (group.Name.Length <= fits)
        {
            return group.Name;
        }

        if (fits <= MinLabelChars)
        {
            return null;
        }

        return group.Name[..(fits - 1)].TrimEnd() + Ellipsis;
    }

    private static string HoverTitle(ProgressGroup group, string unit)
    {
        var exact = ProgressText.Exact(group.Done + group.Partial, group.Total);
        var title = $"{group.Name}: {exact}% \u2014 {ProgressText.Amount(group.Done)}{unit} done";

        if (group.Partial > 0)
        {
            title += $", {ProgressText.Amount(group.Partial)}{unit} through a fallback or stand-in";
        }

        title += $", {ProgressText.Amount(group.Todo)}{unit} not yet";

        if (!string.IsNullOrEmpty(group.Note))
        {
            title += ". " + group.Note;
        }

        return title;
    }
}
