// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Xml;
using System.Xml.Linq;

namespace FiveStar.Architecture.Tests.Checking;

/// <summary>A <c>.csproj</c>: its name (the root namespace), its folder and the projects it references.</summary>
public sealed record ProjectFile(string Path, string Name, string Folder, IReadOnlyList<ProjectReference> References)
{
    public static ProjectFile Parse(SourceFile file)
    {
        var document = XDocument.Parse(file.Text ?? string.Empty, LoadOptions.SetLineInfo);
        var references = document
            .Descendants()
            .Where(element => element.Name.LocalName == "ProjectReference")
            .Select(ToReference)
            .ToArray();

        var slash = file.Path.LastIndexOf('/');
        var folder = slash < 0 ? string.Empty : file.Path[..slash];

        return new ProjectFile(file.Path, file.Name, folder, references);
    }

    /// <summary>Whether <paramref name="path"/> lies below this project's folder.</summary>
    public bool Contains(string path)
    {
        if (Folder.Length == 0)
        {
            return true;
        }

        return path.StartsWith(Folder + "/", StringComparison.Ordinal);
    }

    private static ProjectReference ToReference(XElement element)
    {
        var include = ((string?)element.Attribute("Include") ?? string.Empty).Replace('\\', '/');
        var line = ((IXmlLineInfo)element).LineNumber;

        return new ProjectReference(System.IO.Path.GetFileNameWithoutExtension(include), line);
    }
}
