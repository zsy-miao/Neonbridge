// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;

namespace FiveStar.Cli.Progress;

/// <summary>Number and percentage text of the report. Percentages round down so nothing unfinished reads 100%.</summary>
internal static class ProgressText
{
    private const double Percent = 100;
    private const double TwoDecimalsFrom = 99.9;
    private const double OneDecimalFrom = 99;
    private const double Tenths = 10;
    private const double Hundredths = 100;
    private const string Complete = "100";

    /// <summary>A count with thousands separators, or an amount with up to two decimals.</summary>
    public static string Amount(double value)
    {
        return value.ToString("#,0.##", CultureInfo.InvariantCulture);
    }

    /// <summary>Headline percentage: two decimals from 99.9%, one below, 100 only when complete.</summary>
    public static string Headline(double finished, double total)
    {
        if (total <= 0 || finished >= total)
        {
            return Complete;
        }

        var share = Percent * finished / total;

        if (share >= TwoDecimalsFrom)
        {
            return Floor(share, Hundredths, "0.00");
        }

        return Floor(share, Tenths, "0.0");
    }

    /// <summary>Tile percentage: whole percent, one decimal from 99%, 100 only when complete.</summary>
    public static string Tile(double finished, double total)
    {
        if (total <= 0 || finished >= total)
        {
            return Complete;
        }

        var share = Percent * finished / total;

        if (share >= OneDecimalFrom)
        {
            return Floor(share, Tenths, "0.0");
        }

        return Floor(share, 1, "0");
    }

    /// <summary>Hover-title percentage: one decimal, 100.0 only when complete.</summary>
    public static string Exact(double finished, double total)
    {
        if (total <= 0 || finished >= total)
        {
            return "100.0";
        }

        return Floor(Percent * finished / total, Tenths, "0.0");
    }

    /// <summary>A coordinate with one decimal.</summary>
    public static string Coordinate(double value)
    {
        return value.ToString("0.0", CultureInfo.InvariantCulture);
    }

    private static string Floor(double share, double scale, string format)
    {
        return (Math.Floor(share * scale) / scale).ToString(format, CultureInfo.InvariantCulture);
    }
}
