// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Architecture.Tests.Checking;

/// <summary>One place where the code breaks a rule; <see cref="Line"/> counts from 1.</summary>
public sealed record Violation(string Path, int Line, string Rule, string Message)
{
    public override string ToString() => $"{Path}:{Line}: [{Rule}] {Message}";
}
