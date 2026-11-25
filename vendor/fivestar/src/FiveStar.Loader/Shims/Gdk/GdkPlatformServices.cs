// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>The GDK services that answer from the PC itself or from nothing: events, display, accessibility, launching, connectivity.</summary>
internal sealed record GdkPlatformServices(
    EventRegistrations Registrations,
    DisplayService Display,
    AccessibilityService Accessibility,
    LauncherService Launcher,
    ConnectivityService Connectivity);
