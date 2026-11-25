// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Cli.Run;

/// <summary>One prerequisite: what was checked, the outcome, and what was found or how to fix it.</summary>
internal sealed record RunCheck(string Name, CheckStatus Status, string Detail)
{
    public static RunCheck Ok(string name, string detail) => new(name, CheckStatus.Ok, detail);

    public static RunCheck Warning(string name, string detail) => new(name, CheckStatus.Warning, detail);

    public static RunCheck Failed(string name, string detail) => new(name, CheckStatus.Failed, detail);
}
