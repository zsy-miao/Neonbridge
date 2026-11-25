// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Cli.Progress;

/// <summary>Draws the still-needed list as side-by-side columns, each item a name with a wrapped note below it.</summary>
internal sealed class NeededSection
{
    private const string Heading = "Still needed";
    private const double HeadingSize = 15;
    private const double HeadingBaseline = 15;
    private const double FirstColumnLine = 38;
    private const double ColumnHeadingSize = 13;
    private const double ColumnHeadingSpace = 18;
    private const double NameSize = 12;
    private const double NameLine = 15;
    private const double NameCharWidth = 6.6;
    private const double NoteSize = 11;
    private const double NoteLine = 14;
    private const double NoteCharWidth = 6.1;
    private const double ItemSpace = 6;
    private const double ColumnGutter = 16;
    private const double SpaceAfter = 4;

    private readonly SvgCanvas _canvas;
    private readonly double _left;
    private readonly double _width;

    public NeededSection(SvgCanvas canvas, double left, double width)
    {
        _canvas = canvas;
        _left = left;
        _width = width;
    }

    /// <summary>Draws the list with its top at <paramref name="y"/> and returns the top of what follows.</summary>
    public double Draw(IReadOnlyList<NeededColumn> columns, double y)
    {
        _canvas.BoldText("ink", _left, y + HeadingBaseline, HeadingSize, Heading);
        y += FirstColumnLine;
        var columnWidth = _width / columns.Count;
        var bottom = y;

        for (var index = 0; index < columns.Count; index++)
        {
            var columnBottom = DrawColumn(columns[index], _left + (index * columnWidth), y, columnWidth - ColumnGutter);
            bottom = Math.Max(bottom, columnBottom);
        }

        return bottom + SpaceAfter;
    }

    private double DrawColumn(NeededColumn column, double x, double y, double width)
    {
        _canvas.BoldText("ink", x, y, ColumnHeadingSize, column.Area);
        y += ColumnHeadingSpace;

        foreach (var item in column.Items)
        {
            foreach (var line in Wrap(item.Name, (int)(width / NameCharWidth)))
            {
                _canvas.Text("ink", x, y, NameSize, line);
                y += NameLine;
            }

            foreach (var line in Wrap(item.Note ?? string.Empty, (int)(width / NoteCharWidth)))
            {
                _canvas.Text("ink2", x, y, NoteSize, line);
                y += NoteLine;
            }

            y += ItemSpace;
        }

        return y;
    }

    /// <summary>Splits text at spaces into lines of at most <paramref name="limit"/> characters (a longer word stays whole).</summary>
    private static List<string> Wrap(string text, int limit)
    {
        var lines = new List<string>();
        var line = string.Empty;

        foreach (var word in text.Split(' ', StringSplitOptions.RemoveEmptyEntries))
        {
            if (line.Length > 0 && line.Length + 1 + word.Length > limit)
            {
                lines.Add(line);
                line = word;
            }
            else if (line.Length > 0)
            {
                line += " " + word;
            }
            else
            {
                line = word;
            }
        }

        if (line.Length > 0)
        {
            lines.Add(line);
        }

        return lines;
    }
}
