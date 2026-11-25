// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>An XPackageInstallationMonitorHandle: the package it watches and the task queue its callbacks run on (0 = process queue).</summary>
internal sealed record InstallationMonitor(string PackageId, nint Queue);
