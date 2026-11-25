// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>
/// Pipelines the driver of this machine cannot build: their compile is expected to take more than the machine's
/// physical memory less the floor (<see cref="CompileBudget.Buildable(long)"/>), so they are never admitted and stay
/// stand-ins (issue #19: a 12 MB compute program on AMD). Each is logged once and recorded with the adapter's vendor and
/// driver version in <c>unbuildable.txt</c> beside the pipeline cache; a record of another vendor or driver does not count.
/// </summary>
internal static class UnbuildablePipelines
{
    /// <summary>The index's file name.</summary>
    public const string FileName = "unbuildable.txt";

    private static readonly object Gate = new();
    private static HashSet<string>? _records;
    private static string? _path;

    /// <summary>Keeps the index in <paramref name="folder"/> (the pipeline cache's), or in memory only without one.</summary>
    public static void OpenIn(string? folder)
    {
        string? path = null;

        if (folder is not null)
        {
            path = Path.Combine(folder, FileName);
        }

        Open(path);
    }

    /// <summary>Sets the index file (null keeps the records in memory only); call before the first build.</summary>
    public static void Open(string? path)
    {
        lock (Gate)
        {
            _path = path;
            _records = null;
        }
    }

    /// <summary>The record of <paramref name="key"/> on an adapter: the key, vendor id and driver version.</summary>
    public static string Record(string key, uint vendorId, long driverVersion)
    {
        return string.Create(CultureInfo.InvariantCulture, $"{key} {vendorId:x4} {driverVersion:x}");
    }

    /// <summary>
    /// Records the pipeline <paramref name="key"/> as unbuildable on this adapter; true the first time (log it then).
    /// </summary>
    public static bool Add(string key)
    {
        string record = Record(key, CompileBudget.VendorId, CompileBudget.DriverVersion);

        lock (Gate)
        {
            var records = Records();

            if (!records.Add(record))
            {
                return false;
            }

            if (_path is not null)
            {
                try
                {
                    File.AppendAllText(_path, record + Environment.NewLine);
                }
                catch (IOException)
                {
                    // The record stays in memory; the estimate alone keeps the pipeline out next time too.
                }
            }

            return true;
        }
    }

    /// <summary>The records of this adapter and driver found in the file, and those added since.</summary>
    public static int Count
    {
        get
        {
            lock (Gate)
            {
                return Records().Count;
            }
        }
    }

    private static HashSet<string> Records()
    {
        if (_records is not null)
        {
            return _records;
        }

        _records = new HashSet<string>(StringComparer.Ordinal);

        if (_path is not null && File.Exists(_path))
        {
            string suffix = Record(string.Empty, CompileBudget.VendorId, CompileBudget.DriverVersion);
            _records.UnionWith(File.ReadAllLines(_path).Where(line => line.EndsWith(suffix, StringComparison.Ordinal)));
        }

        return _records;
    }
}
