// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Architecture.Tests.Checking.Rules;

/// <summary>At most one top-level type per file, named like the file.</summary>
public sealed class OneTypePerFileRule : SyntaxRule
{
    public override string Name => "OneTypePerFile";

    protected override IEnumerable<Violation> Check(CSharpSource source)
    {
        var types = TypeDeclarations.TopLevel(source.Root);

        for (var index = 1; index < types.Count; index++)
        {
            yield return source.Violation(Name, types[index], $"a second top-level type `{TypeDeclarations.NameOf(types[index])}`");
        }

        if (types.Count == 1 && TypeDeclarations.NameOf(types[0]) != source.File.Name)
        {
            var message = $"the type `{TypeDeclarations.NameOf(types[0])}` is not named like the file `{source.File.Name}`";

            yield return source.Violation(Name, types[0], message);
        }
    }
}
