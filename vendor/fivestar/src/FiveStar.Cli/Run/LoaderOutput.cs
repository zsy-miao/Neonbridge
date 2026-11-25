// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using System.Text.RegularExpressions;

namespace FiveStar.Cli.Run;

/// <summary>
/// Reads the loader's console lines as they come: keeps the milestones (lines starting <c>==</c>, errors, faults with
/// their address line, presents, the exit line) for the console and counts presents, faults and warnings for the
/// summary. Everything else stays in the log file.
/// </summary>
internal sealed partial class LoaderOutput
{
    private const string MilestonePrefix = "==";
    private const string ErrorPrefix = "ERROR ";
    private const string WarningPrefix = "WARN  ";
    private const string AddressPrefix = "  at ";
    private const string ExitPrefix = "done, exit code";
    private const string PresentTag = "[present]";

    private bool _afterFault;

    /// <summary>The highest present number the loader reported.</summary>
    public long Presents { get; private set; }

    /// <summary>The highest fault number the loader reported.</summary>
    public int Faults { get; private set; }

    public int Warnings { get; private set; }

    /// <summary>Counts the line; true when it belongs on the console.</summary>
    public bool Observe(string line)
    {
        if (_afterFault && line.StartsWith(AddressPrefix, StringComparison.Ordinal))
        {
            _afterFault = false;

            return true;
        }

        if (line.StartsWith(WarningPrefix, StringComparison.Ordinal))
        {
            Warnings++;

            return false;
        }

        var fault = FaultPattern().Match(line);

        if (fault.Success)
        {
            Faults = Math.Max(Faults, int.Parse(fault.Groups[1].Value, CultureInfo.InvariantCulture));
            _afterFault = true;

            return true;
        }

        var present = PresentPattern().Match(line);

        if (present.Success)
        {
            Presents = Math.Max(Presents, long.Parse(present.Groups[1].Value, CultureInfo.InvariantCulture));

            return true;
        }

        return line.StartsWith(MilestonePrefix, StringComparison.Ordinal)
            || line.StartsWith(ErrorPrefix, StringComparison.Ordinal)
            || line.StartsWith(ExitPrefix, StringComparison.Ordinal)
            || line.Contains(PresentTag, StringComparison.Ordinal);
    }

    /// <summary>FaultRecordWriter's first line: <c>FAULT #n ...</c>.</summary>
    [GeneratedRegex(@"FAULT #(\d+)")]
    private static partial Regex FaultPattern();

    /// <summary>Presenter's lines: <c>[present] present n at ...</c> and <c>[present] n presents, ...</c>.</summary>
    [GeneratedRegex(@"\[present\] (?:present )?(\d+)")]
    private static partial Regex PresentPattern();
}
