// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Architecture.Tests.Checking;

/// <summary>The files a check reads: every file, the project files among them and the parsed C# files.</summary>
public sealed class CodeBase
{
    public CodeBase(IReadOnlyList<SourceFile> files)
    {
        Files = files;
        Projects = files
            .Where(file => file.Extension == ".csproj" && file.Text is not null)
            .Select(ProjectFile.Parse)
            .ToArray();

        CSharpFiles = files
            .Where(file => file.Extension == ".cs" && file.Text is not null)
            .Select(file => new CSharpSource(file, ProjectOf(file.Path)))
            .ToArray();
    }

    public IReadOnlyList<SourceFile> Files { get; }

    public IReadOnlyList<ProjectFile> Projects { get; }

    public IReadOnlyList<CSharpSource> CSharpFiles { get; }

    /// <summary>The innermost project whose folder holds the path, if any.</summary>
    public ProjectFile? ProjectOf(string path) =>
        Projects
            .Where(project => project.Contains(path))
            .OrderByDescending(project => project.Folder.Length)
            .FirstOrDefault();
}
