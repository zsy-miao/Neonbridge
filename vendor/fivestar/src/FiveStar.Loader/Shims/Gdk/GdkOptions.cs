// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Shims.Network;

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>Settings of the offline GDK.</summary>
/// <param name="PackageRoot">Where MicrosoftGame.config is read from; null means the main image's directory.</param>
/// <param name="SaveRoot">Root of saves and local storage; null means <paramref name="DefaultDataRoot"/>\&lt;TitleId&gt;.</param>
/// <param name="DefaultDataRoot">Per-user data folder used when no save root is given (outside any repository).</param>
/// <param name="FallbackPackageRoot">Package root when no image is mapped (the current directory).</param>
/// <param name="DeviceType">What XSystemGetDeviceType returns.</param>
/// <param name="OsVersion">The host OS version, reported by XSystemGetAnalyticsInfo.</param>
/// <param name="Network">What XNetworking reports (<c>--network</c>).</param>
internal sealed record GdkOptions(
    string? PackageRoot,
    string? SaveRoot,
    string DefaultDataRoot,
    string FallbackPackageRoot,
    uint DeviceType,
    Version OsVersion,
    NetworkOptions Network);
