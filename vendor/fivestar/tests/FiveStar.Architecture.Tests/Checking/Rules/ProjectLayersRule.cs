// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Architecture.Tests.Checking.Rules;

/// <summary>Every project sits in a layer of the policy and references only what its layer allows; a test project only its subject.</summary>
public sealed class ProjectLayersRule : IRule
{
    private const int ProjectLine = 1;

    public string Name => "ProjectLayers";

    public IReadOnlyList<Violation> Check(CodeBase codeBase)
    {
        var violations = new List<Violation>();

        foreach (var project in codeBase.Projects)
        {
            var allowed = AllowedReferences(project.Name);

            if (allowed is null)
            {
                violations.Add(new Violation(project.Path, ProjectLine, Name, $"project `{project.Name}` is in no layer"));
                continue;
            }

            var forbidden = project.References.Where(reference => !allowed.Contains(reference.Name, StringComparer.Ordinal));

            foreach (var reference in forbidden)
            {
                violations.Add(new Violation(project.Path, reference.Line, Name, $"`{project.Name}` may not reference `{reference.Name}`"));
            }
        }

        return violations;
    }

    /// <summary>The projects a project may reference, or null when it is in no layer.</summary>
    private static IReadOnlyList<string>? AllowedReferences(string project)
    {
        if (project.EndsWith(CodePolicy.TestProjectSuffix, StringComparison.Ordinal))
        {
            return [project[..^CodePolicy.TestProjectSuffix.Length]];
        }

        if (CodePolicy.Layers.TryGetValue(project, out var allowed))
        {
            return allowed;
        }

        return null;
    }
}
