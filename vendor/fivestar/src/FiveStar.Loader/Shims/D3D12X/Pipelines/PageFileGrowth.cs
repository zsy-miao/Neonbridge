// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using Microsoft.Win32;

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>
/// How far Windows can still raise the commit limit by growing the page files (<see cref="WarmMemoryGuard"/>). A
/// page file grows up to its maximum when commit nears the limit: the configured maximum (PagingFiles in the Memory
/// Management key: "path min max" in MB), or for a system-managed one ("?:\pagefile.sys", or min and max 0) three times
/// the physical memory. Growth also needs free space on the page file's drive, of which <see cref="DiskMarginBytes"/>
/// stays free. A machine whose page files cannot grow gets 0, and the guard then works on the current limit alone.
/// </summary>
internal static class PageFileGrowth
{
    /// <summary>Disk space left free on a page file's drive.</summary>
    public const ulong DiskMarginBytes = 16UL << 30;

    private const string MemoryManagementKey = @"SYSTEM\CurrentControlSet\Control\Session Manager\Memory Management";
    private const string PagingFilesValue = "PagingFiles";
    private const int SystemManagedFactor = 3;
    private const int MegabyteShift = 20;
    private const int MinimumField = 1;
    private const int MaximumField = 2;
    private const char AnyDrive = '?';
    private const int DriveAndColon = 2;

    /// <summary>The bytes the commit limit can still grow by; <paramref name="pageFileBytes"/> is the page files' size now.</summary>
    public static ulong Headroom(ulong physicalBytes, ulong pageFileBytes)
    {
        if (!OperatingSystem.IsWindows())
        {
            return 0;
        }

        try
        {
            var entries = Registry.LocalMachine.OpenSubKey(MemoryManagementKey)?.GetValue(PagingFilesValue) as string[];

            return Headroom(entries ?? [], physicalBytes, pageFileBytes, FreeBytes);
        }
        catch (Exception exception) when (exception is UnauthorizedAccessException or IOException or System.Security.SecurityException or FormatException or OverflowException)
        {
            return 0;
        }
    }

    /// <summary>
    /// The growth room of the page files in <paramref name="entries"/> (PagingFiles strings), given the physical memory,
    /// the page files' current size and the free space of a drive (<paramref name="freeBytes"/>, by drive letter).
    /// </summary>
    public static ulong Headroom(IReadOnlyList<string> entries, ulong physicalBytes, ulong pageFileBytes, Func<char, ulong> freeBytes)
    {
        ulong maximum = 0;
        ulong free = 0;
        var drives = new HashSet<char>();

        foreach (var entry in entries)
        {
            var fields = entry.Split(' ', StringSplitOptions.RemoveEmptyEntries);

            if (fields.Length == 0 || fields[0].Length < DriveAndColon)
            {
                continue;
            }

            var drive = char.ToUpperInvariant(fields[0][0]);

            if (drive == AnyDrive)
            {
                drive = char.ToUpperInvariant(Environment.GetFolderPath(Environment.SpecialFolder.Windows)[0]);
            }

            maximum += Maximum(fields, physicalBytes);

            if (drives.Add(drive))
            {
                var space = freeBytes(drive);

                if (space > DiskMarginBytes)
                {
                    free += space - DiskMarginBytes;
                }
            }
        }

        if (maximum <= pageFileBytes)
        {
            return 0;
        }

        return Math.Min(maximum - pageFileBytes, free);
    }

    /// <summary>A page file's maximum: the configured one, or three times the physical memory when system-managed.</summary>
    private static ulong Maximum(string[] fields, ulong physicalBytes)
    {
        if (fields[0][0] == AnyDrive || fields.Length <= MaximumField)
        {
            return physicalBytes * SystemManagedFactor;
        }

        var minimum = ulong.Parse(fields[MinimumField], CultureInfo.InvariantCulture);
        var maximum = ulong.Parse(fields[MaximumField], CultureInfo.InvariantCulture);

        if (minimum == 0 && maximum == 0)
        {
            return physicalBytes * SystemManagedFactor;
        }

        return maximum << MegabyteShift;
    }

    private static ulong FreeBytes(char drive)
    {
        try
        {
            return (ulong)new DriveInfo(drive.ToString()).AvailableFreeSpace;
        }
        catch (Exception exception) when (exception is IOException or ArgumentException or UnauthorizedAccessException)
        {
            return 0;
        }
    }
}
