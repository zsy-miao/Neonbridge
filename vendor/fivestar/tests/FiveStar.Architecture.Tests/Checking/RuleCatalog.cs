// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Architecture.Tests.Checking.Rules;

namespace FiveStar.Architecture.Tests.Checking;

/// <summary>Every rule the repository is checked against, in the order of CONTRIBUTING.md.</summary>
public static class RuleCatalog
{
    public static readonly IReadOnlyList<IRule> All =
    [
        new FileHeaderRule(),
        new OneTypePerFileRule(),
        new NamespaceFollowsFolderRule(),
        new TypeModifiersRule(),
        new BlockSpacingRule(),
        new StatementGroupingRule(),
        new MethodLengthRule(),
        new ConditionalExpressionRule(),
        new PropertyPatternRule(),
        new MagicNumberRule(),
        new HotPathAllocationRule(),
        new AmbientStateRule(),
        new ConstructorShapeRule(),
        new ConstructorSubscriptionRule(),
        new ProjectLayersRule(),
        new ForbiddenWordsRule(),
        new UnmanagedCallersGuardRule(),
    ];

    public static IRule Find(string name) => All.Single(rule => rule.Name == name);
}
