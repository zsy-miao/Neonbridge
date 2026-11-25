// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;

namespace FiveStar.Cli.Setup;

/// <summary>The words setup prints for conditions, durations and sizes.</summary>
internal static class StepText
{
    private const double BytesPerMebibyte = 1 << 20;
    private const double BytesPerGibibyte = 1 << 30;
    private const string OneDecimal = "F1";

    public static string Describe(StepCondition condition) => condition switch
    {
        StepCondition.Done => "done",
        StepCondition.NotRun => "not run",
        StepCondition.Interrupted => "interrupted",
        StepCondition.InputsChanged => "stale: inputs changed",
        StepCondition.ToolChanged => "stale: new tool build",
        StepCondition.OutputsMissing => "stale: outputs missing",
        StepCondition.Waiting => "waiting for an earlier step",
        _ => condition.ToString(),
    };

    public static string Duration(TimeSpan time)
    {
        return time.ToString(@"h\:mm\:ss", CultureInfo.InvariantCulture);
    }

    public static string Size(long bytes)
    {
        if (bytes < BytesPerGibibyte)
        {
            return (bytes / BytesPerMebibyte).ToString(OneDecimal, CultureInfo.InvariantCulture) + " MiB";
        }

        return (bytes / BytesPerGibibyte).ToString(OneDecimal, CultureInfo.InvariantCulture) + " GiB";
    }

    /// <summary>The total size of the files below a folder; 0 when it does not exist.</summary>
    public static long FolderSize(string folder)
    {
        if (!Directory.Exists(folder))
        {
            return 0;
        }

        return new DirectoryInfo(folder).EnumerateFiles("*", SearchOption.AllDirectories).Sum(file => file.Length);
    }
}
