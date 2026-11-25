// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Architecture.Tests.Checking;

/// <summary>A rule that reads each C# file on its own.</summary>
public abstract class SyntaxRule : IRule
{
    public abstract string Name { get; }

    public IReadOnlyList<Violation> Check(CodeBase codeBase) =>
        codeBase.CSharpFiles.Where(Applies).SelectMany(Check).ToArray();

    /// <summary>Whether the rule reads this file; every file by default.</summary>
    protected virtual bool Applies(CSharpSource source) => true;

    protected abstract IEnumerable<Violation> Check(CSharpSource source);
}
