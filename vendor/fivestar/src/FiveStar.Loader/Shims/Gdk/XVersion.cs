// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>XVersion (XGameRuntimeTypes.h): major, minor, build and revision as 16-bit fields of one uint64_t, major highest.</summary>
internal static class XVersion
{
    private const int MajorShift = 48;
    private const int MinorShift = 32;
    private const int BuildShift = 16;

    public static ulong Pack(Version version)
    {
        ulong major = (ushort)version.Major;
        ulong minor = (ushort)version.Minor;
        ulong build = (ushort)Math.Max(0, version.Build);
        ulong revision = (ushort)Math.Max(0, version.Revision);

        return (major << MajorShift) | (minor << MinorShift) | (build << BuildShift) | revision;
    }
}
