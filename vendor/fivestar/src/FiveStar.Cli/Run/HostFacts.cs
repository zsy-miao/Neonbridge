// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Cli.Run;

/// <summary>The facts of this PC the prerequisite checks read: the platform, System32 and the 32-bit Program Files folder.</summary>
internal sealed record HostFacts(bool WindowsX64, string SystemFolder, string ProgramFilesX86)
{
    public static HostFacts Current() =>
        new(
            OperatingSystem.IsWindows() && RuntimeInformation.OSArchitecture == Architecture.X64,
            Environment.SystemDirectory,
            Environment.GetFolderPath(Environment.SpecialFolder.ProgramFilesX86));
}
