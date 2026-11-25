// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Architecture.Tests.Checking;

/// <summary>A <c>ProjectReference</c> of a project file: the referenced project's name and the line it is on.</summary>
public sealed record ProjectReference(string Name, int Line);
