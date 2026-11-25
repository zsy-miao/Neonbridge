// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Architecture.Tests.Checking;

/// <summary>One code rule: its name and the violations it finds in a code base.</summary>
public interface IRule
{
    string Name { get; }

    IReadOnlyList<Violation> Check(CodeBase codeBase);
}
